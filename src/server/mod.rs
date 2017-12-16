use bifrost::rpc;
use bifrost::conshash::ConsistentHashing;
use bifrost::raft;
use bifrost::raft::client::RaftClient;
use bifrost::raft::state_machine::{master as sm_master};
use bifrost::membership::server::Membership;
use bifrost::membership::member::MemberService;
use bifrost::conshash::weights::Weights;
use bifrost::tcp::{STANDALONE_ADDRESS_STRING};
use ram::chunk::Chunks;
use ram::schema::SchemasServer;
use ram::schema::{sm as schema_sm};
use ram::types::Id;
use std::sync::Arc;
use std::io;

pub mod cell_rpc;
pub mod transactions;

#[derive(Debug)]
pub enum ServerError {
    CannotJoinCluster,
    CannotJoinClusterGroup(sm_master::ExecError),
    CannotInitMemberTable,
    CannotSetServerWeight,
    CannotInitConsistentHashTable,
    CannotLoadMetaClient,
    CannotInitializeSchemaServer(sm_master::ExecError),
    StandaloneMustAlsoBeMetaServer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerOptions {
    pub chunk_count: usize,
    pub memory_size: usize,
    pub standalone: bool,
    pub is_meta: bool,
    pub meta_members: Vec<String>,
    pub address: String,
    pub backup_storage: Option<String>,
    pub meta_storage: Option<String>,
    pub group_name: String,
}

pub struct ServerMeta {
    pub schemas: SchemasServer
}

pub struct NebServer {
    pub chunks: Arc<Chunks>,
    pub meta: Arc<ServerMeta>,
    pub rpc: Arc<rpc::Server>,
    pub consh: Arc<ConsistentHashing>,
    pub member_pool: rpc::ClientPool,
    pub txn_peer: transactions::Peer,
    pub raft_service: Option<Arc<raft::RaftService>>,
    pub server_id: u64
}

impl NebServer {
    fn load_meta_server(opt: &ServerOptions, rpc_server: &Arc<rpc::Server>) -> Result<Arc<raft::RaftService>, ServerError> {
        let server_addr = &opt.address;
        let raft_service = raft::RaftService::new(raft::Options {
            storage: match opt.meta_storage {
                Some(ref path) => raft::Storage::DISK(path.clone()),
                None => raft::Storage::MEMORY,
            },
            address: server_addr.clone(),
            service_id: raft::DEFAULT_SERVICE_ID,
        });
        rpc_server.register_service(
            raft::DEFAULT_SERVICE_ID,
            &raft_service
        );
        raft_service.register_state_machine(Box::new(schema_sm::SchemasSM::new(&opt.group_name, &raft_service)));
        raft::RaftService::start(&raft_service);
        match raft_service.join(&opt.meta_members) {
            Err(sm_master::ExecError::CannotConstructClient) => {
                info!("Cannot join meta cluster, will bootstrap one.");
                raft_service.bootstrap();
            },
            Ok(Ok(())) => {
                info!("Joined meta cluster, number of members: {}", raft_service.num_members());
            },
            e => {
                error!("Cannot join into cluster: {:?}", e);
                return Err(ServerError::CannotJoinCluster)
            }
        }
        Membership::new(rpc_server, &raft_service);
        Weights::new(&raft_service);
        return Ok(raft_service);
    }
    fn join_group(opt: &ServerOptions, raft_client: &Arc<RaftClient>) -> Result<(), ServerError> {
        let member_service = MemberService::new(&opt.address, raft_client);
        match member_service.join_group(&opt.group_name) {
            Ok(_) => Ok(()),
            Err(e) => {
                error!("Cannot join cluster group");
                Err(ServerError::CannotJoinClusterGroup(e))
            }
        }
    }
    fn init_conshash(opt: &ServerOptions, raft_client: &Arc<RaftClient>)
        -> Result<Arc<ConsistentHashing>, ServerError> {
        match ConsistentHashing::new(&opt.group_name, raft_client) {
            Ok(ch) => {
                ch.set_weight(&opt.address, opt.memory_size as u64);
                if !ch.init_table().is_ok() {
                    error!("Cannot initialize member table");
                    return Err(ServerError::CannotInitMemberTable);
                }
                return Ok(ch);
            },
            _ => {
                error!("Cannot initialize consistent hash table");
                return Err(ServerError::CannotInitConsistentHashTable);
            }
        }
    }
    fn load_cluster_clients (
        opt: &ServerOptions,
        schemas: &mut SchemasServer,
        rpc_server: &Arc<rpc::Server>
    ) -> Result<Arc<ConsistentHashing>, ServerError> {
        let raft_client =
            RaftClient::new(&opt.meta_members, raft::DEFAULT_SERVICE_ID);
        match raft_client {
            Ok(raft_client) => {
                RaftClient::prepare_subscription(rpc_server);
                NebServer::join_group(opt, &raft_client)?;
                let conshash = NebServer::init_conshash(opt, &raft_client)?;
                let schema_server = match SchemasServer::new(&opt.group_name, Some(&raft_client)) {
                    Ok(schema) => schema,
                    Err(e) => return Err(ServerError::CannotInitializeSchemaServer(e))
                };
                *schemas = schema_server;
                Ok(conshash)
            },
            Err(e) => {
                error!("Cannot load meta client: {:?}", e);
                Err(ServerError::CannotLoadMetaClient)
            }
        }
    }
    pub fn new_from_opts(opts: &ServerOptions) -> Result<Arc<NebServer>, ServerError> {
        let server_addr = if opts.standalone {&STANDALONE_ADDRESS_STRING} else {&opts.address};
        let rpc_server = rpc::Server::new(server_addr);
        rpc::Server::listen_and_resume(&rpc_server);
        return NebServer::new(opts, server_addr, &rpc_server)
    }
    pub fn new(
        opts: &ServerOptions,
        server_addr: &String,
        rpc_server: &Arc<rpc::Server>,
    ) -> Result<Arc<NebServer>, ServerError> {
        let mut raft_service = None;
        if opts.is_meta {
            raft_service = Some(NebServer::load_meta_server(&opts, &rpc_server)?);
        }
        if !opts.is_meta && opts.standalone {
            return Err(ServerError::StandaloneMustAlsoBeMetaServer)
        }
        let mut schemas = SchemasServer::new(&opts.group_name, None).unwrap();
        let conshasing =
            NebServer::load_cluster_clients(&opts, &mut schemas, &rpc_server)?;
        let meta_rc = Arc::new(ServerMeta {
            schemas
        });
        let chunks = Chunks::new(
            opts.chunk_count,
            opts.memory_size,
            meta_rc.clone(),
            opts.backup_storage.clone(),
        );
        let server = Arc::new(NebServer {
            chunks,
            meta: meta_rc,
            rpc: rpc_server.clone(),
            consh: conshasing,
            member_pool: rpc::ClientPool::new(),
            txn_peer: transactions::Peer::new(server_addr),
            raft_service,
            server_id: rpc_server.server_id
        });
        rpc_server.register_service(
            cell_rpc::DEFAULT_SERVICE_ID,
            &cell_rpc::NebRPCService::new(&server)
        );
        rpc_server.register_service(
            transactions::manager::DEFAULT_SERVICE_ID,
            &transactions::manager::TransactionManager::new(&server)
        );
        rpc_server.register_service(
            transactions::data_site::DEFAULT_SERVICE_ID,
            &transactions::data_site::DataManager::new(&server)
        );
        Ok(server)
    }
    pub fn get_server_id_by_id(&self, id: &Id) -> Option<u64> {
        if let Some(server_id) = self.consh.get_server_id(id.higher) {
            Some(server_id)
        } else {
            None
        }
    }
    pub fn get_member_by_server_id(&self, server_id: u64) -> io::Result<Arc<rpc::RPCClient>> {
        if let Some(ref server_name) = self.consh.to_server_name(Some(server_id)) {
            self.member_pool.get(server_name)
        } else {
            Err(io::Error::new(io::ErrorKind::Other, "Cannot find server id in consistent hash table"))
        }
    }
}