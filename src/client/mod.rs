use std::sync::Arc;
use std::io;
use std::cell::{Cell as StdCell};
use bifrost::conshash::{ConsistentHashing, CHError};
use bifrost::raft::client::{RaftClient, ClientError};
use bifrost::raft;
use bifrost::rpc::{RPCError, DEFAULT_CLIENT_POOL, Server as RPCServer};
use bifrost::raft::state_machine::master::ExecError;
use bifrost::raft::state_machine::callback::server::{NotifyError};

use server::{transactions as txn_server, cell_rpc as plain_server};
use ram::types::Id;
use ram::cell::{Cell, Header, ReadError, WriteError};
use ram::schema::{sm as schema_sm};
use ram::schema::sm::client::{SMClient as SchemaClient};
use ram::schema::Schema;

use futures::Future;
use futures::prelude::*;

use self::transaction::*;

static TRANSACTION_MAX_RETRY: u32 = 500;

pub mod transaction;

#[derive(Debug)]
pub enum NebClientError {
    RaftClientError(ClientError),
    ConsistentHashtableError(CHError)
}

struct AsyncClientInner {
    pub conshash: Arc<ConsistentHashing>,
    pub raft_client: Arc<RaftClient>,
    pub schema_client: SchemaClient
}

impl AsyncClientInner {
    pub fn new<'a>(subscription_server: &Arc<RPCServer>, meta_servers: &Vec<String>, group: &'a str)
        -> Result<AsyncClientInner, NebClientError>
    {
        match RaftClient::new(meta_servers, raft::DEFAULT_SERVICE_ID) {
            Ok(raft_client) => {
                RaftClient::prepare_subscription(subscription_server);
                assert!(RaftClient::can_callback());
                match ConsistentHashing::new_client(group, &raft_client) {
                    Ok(chash) => Ok(AsyncClientInner {
                        conshash: chash,
                        raft_client: raft_client.clone(),
                        schema_client: SchemaClient::new(schema_sm::generate_sm_id(group), &raft_client)
                    }),
                    Err(err) => Err(NebClientError::ConsistentHashtableError(err))
                }
            },
            Err(err) => Err(NebClientError::RaftClientError(err))
        }
    }
    pub fn locate_server_address(&self, id: &Id) -> Result<String, RPCError> {
        match self.conshash.get_server(id.higher) {
            Some(n) => Ok(n),
            None => Err(RPCError::IOError(io::Error::new(io::ErrorKind::NotFound, "cannot locate")))
        }
    }
    #[async]
    pub fn locate_plain_server(this: Arc<Self>, id: Id) -> Result<Arc<plain_server::AsyncServiceClient>, RPCError> {
        let address = this.locate_server_address(&id)?;
        let client = match await!(DEFAULT_CLIENT_POOL.get_async(&address)) {
            Ok(c) => c,
            Err(e) => return Err(RPCError::IOError(e))
        };
        Ok(plain_server::AsyncServiceClient::new(plain_server::DEFAULT_SERVICE_ID, &client))
    }
    #[async]
    pub fn read_cell(this: Arc<Self>, id: Id) -> Result<Result<Cell, ReadError>, RPCError> {
        let client = await!(Self::locate_plain_server(this, id))?;
        await!(client.read_cell(&id))
    }
    #[async]
    pub fn write_cell(this: Arc<Self>, cell: Cell) -> Result<Result<Header, WriteError>, RPCError> {
        let client = await!(Self::locate_plain_server(this, cell.id()))?;
        await!(client.write_cell(&cell))
    }
    #[async]
    pub fn update_cell(this: Arc<Self>, cell: Cell) -> Result<Result<Header, WriteError>, RPCError> {
        let client = await!(Self::locate_plain_server(this, cell.id()))?;
        await!(client.update_cell(&cell))
    }
    #[async]
    pub fn remove_cell(this: Arc<Self>, id: Id) -> Result<Result<(), WriteError>, RPCError> {
        let client = await!(Self::locate_plain_server(this, id))?;
        await!(client.remove_cell(&id))
    }
    #[async]
    pub fn transaction<TFN, TR>(this: Arc<Self>, func: TFN) -> Result<TR, TxnError>
        where TFN: Fn(&Transaction) -> Result<TR, TxnError>, TR: 'static, TFN: 'static
    {
        let server_name = match this.conshash.rand_server() {
            Some(name) => name,
            None => return Err(TxnError::CannotFindAServer)
        };
        let txn_client = match txn_server::new_async_client(&server_name) {
            Ok(client) => client,
            Err(e) => return Err(TxnError::IoError(e))
        };
        let mut txn_id: txn_server::TxnId;
        let mut retried = 0;
        while retried < TRANSACTION_MAX_RETRY {
            txn_id = match await!(txn_client.begin()) {
                Ok(Ok(id)) => id,
                _ => return Err(TxnError::CannotBegin)
            };
            let txn = Transaction{
                tid: txn_id,
                state: StdCell::new(txn_server::TxnState::Started),
                client: txn_client.clone(),
            };
            let exec_result = func(&txn);
            let mut exec_value = None;
            let mut txn_result = Ok(());
            match exec_result {
                Ok(val) => {
                    if txn.state.get() == txn_server::TxnState::Started {
                        txn_result = txn.prepare();
                        debug!("PREPARE STATE: {:?}", txn_result);
                    }
                    if txn_result.is_ok() && txn.state.get() == txn_server::TxnState::Prepared {
                        txn_result = txn.commit();
                        debug!("COMMIT STATE: {:?}", txn_result);
                    }
                    exec_value = Some(val);
                },
                Err(e) => {
                    txn_result = Err(e)
                }
            }
            debug!("TXN CONCLUSION: {:?}", txn_result);
            match txn_result {
                Ok(()) => {
                    return Ok(exec_value.unwrap());
                },
                Err(TxnError::NotRealizable) => {
                    let abort_result = txn.abort();  // continue the loop to retry
                    debug!("TXN NOT REALIZABLE, ABORT: {:?}", abort_result);
                },
                Err(e) => {
                    // abort will always be an error to achieve early break
                    let abort_result = txn.abort();
                    debug!("TXN ERROR, ABORT: {:?}", abort_result);
                    return Err(e);
                }
            }
            retried += 1;
            debug!("Client retry transaction, {:?} times", retried);
        }
        Err(TxnError::TooManyRetry)
    }
    #[async]
    pub fn new_schema_with_id(this: Arc<Self>, schema: Schema) -> Result<Result<(), NotifyError>, ExecError> {
        this.schema_client.new_schema(&schema)
    }
    #[async]
    pub fn new_schema(this: Arc<Self>, mut schema: Schema) -> Result<Result<u32, NotifyError>, ExecError> {
        let schema_id = this.schema_client.next_id()?.unwrap();
        schema.id = schema_id;
        await!(Self::new_schema_with_id(this, schema)).map(|r| r.map(|_| schema_id))
    }
    #[async]
    pub fn del_schema(this: Arc<Self>, name: String) -> Result<Result<(), NotifyError>, ExecError> {
        this.schema_client.del_schema(&name)
    }
    #[async]
    pub fn get_all_schema(this: Arc<Self>) -> Result<Vec<Schema>, ExecError> {
        Ok(this.schema_client.get_all()?.unwrap())
    }
}

pub struct AsyncClient {
    inner: Arc<AsyncClientInner>
}

impl AsyncClient {
    pub fn new<'a>(subscription_server: &Arc<RPCServer>, meta_servers: &Vec<String>, group: &'a str)
                   -> Result<AsyncClient, NebClientError>
    {
        AsyncClientInner::new(subscription_server, meta_servers, group)
            .map(|inner| {
                AsyncClient {
                    inner: Arc::new(inner)
                }
            })
    }

    pub fn locate_server_address(&self, id: &Id) -> Result<String, RPCError> {
        self.inner.locate_server_address(id)
    }

    pub fn locate_plain_server(&self, id: Id)
        -> impl Future<Item = Arc<plain_server::AsyncServiceClient>, Error = RPCError>
    {
        AsyncClientInner::locate_plain_server(self.inner.clone(), id)
    }

    pub fn read_cell(&self, id: Id)
        -> impl Future<Item = Result<Cell, ReadError>, Error = RPCError>
    {
        AsyncClientInner::read_cell(self.inner.clone(), id)
    }

    pub fn write_cell(&self, cell: Cell)
        -> impl Future<Item = Result<Header, WriteError>, Error = RPCError>
    {
        AsyncClientInner::write_cell(self.inner.clone(), cell)
    }

    pub fn update_cell(&self, cell: Cell)
        -> impl Future<Item = Result<Header, WriteError>, Error = RPCError>
    {
        AsyncClientInner::update_cell(self.inner.clone(), cell)
    }

    pub fn remove_cell(&self, id: Id)
        -> impl Future<Item = Result<(), WriteError>, Error = RPCError>
    {
        AsyncClientInner::remove_cell(self.inner.clone(), id)
    }

    pub fn transaction<TFN, TR>(&self, func: TFN)
        -> impl Future<Item = TR, Error = TxnError>
        where TFN: Fn(&Transaction) -> Result<TR, TxnError>, TR: 'static, TFN: 'static
    {
        AsyncClientInner::transaction(self.inner.clone(), func)
    }
    pub fn new_schema_with_id(&self, schema: Schema)
        -> impl Future<Item = Result<(), NotifyError>, Error = ExecError>
    {
        AsyncClientInner::new_schema_with_id(self.inner.clone(), schema)
    }
    pub fn new_schema(&self, schema: Schema)
        -> impl Future<Item = Result<u32, NotifyError>, Error = ExecError>
    {
        AsyncClientInner::new_schema(self.inner.clone(), schema)
    }
    pub fn del_schema(&self, name: String)
        -> impl Future<Item = Result<(), NotifyError>, Error = ExecError>
    {
        AsyncClientInner::del_schema(self.inner.clone(), name)
    }
    pub fn get_all_schema(&self)
        -> impl Future<Item = Vec<Schema>, Error = ExecError>
    {
        AsyncClientInner::get_all_schema(self.inner.clone())
    }

    pub fn raft_client(&self) -> Arc<RaftClient> {
        self.inner.raft_client.clone()
    }
}