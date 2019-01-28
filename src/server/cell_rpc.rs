use ram::cell::{Cell, CellHeader, ReadError, WriteError};
use ram::types::Id;
use server::NebServer;
use bifrost::rpc::*;

use futures_cpupool::{CpuPool};
use num_cpus;

pub static DEFAULT_SERVICE_ID: u64 = hash_ident!(NEB_CELL_RPC_SERVICE) as u64;

service! {
    rpc read_cell(key: Id) -> Cell | ReadError;
    rpc write_cell(cell: Cell) -> CellHeader | WriteError;
    rpc update_cell(cell: Cell) -> CellHeader | WriteError;
    rpc remove_cell(key: Id) -> () | WriteError;
}

pub struct NebRPCService {
    inner: Arc<NebRPCServiceInner>
}

pub struct NebRPCServiceInner {
    server: Arc<NebServer>,
    pool: CpuPool
}

impl Service for NebRPCService {
    fn read_cell(&self, key: Id) -> Box<Future<Item = Cell, Error = ReadError>> {
        NebRPCServiceInner::read_cell(self.inner.clone(), key)
    }
    fn write_cell(&self, mut cell: Cell) -> Box<Future<Item =CellHeader, Error = WriteError>> {
        NebRPCServiceInner::write_cell(self.inner.clone(), cell)
    }
    fn update_cell(&self, mut cell: Cell) -> Box<Future<Item =CellHeader, Error = WriteError>> {
        NebRPCServiceInner::update_cell(self.inner.clone(), cell)
    }
    fn remove_cell(&self, key: Id) -> Box<Future<Item = (), Error = WriteError>> {
        NebRPCServiceInner::remove_cell(self.inner.clone(), key)
    }
}

impl NebRPCServiceInner {
    fn read_cell(this: Arc<Self>, key: Id)
        -> Box<Future<Item = Cell, Error = ReadError>>
    {
        box this.clone().pool.spawn_fn(move || this.server.chunks.read_cell(&key))
    }
    fn write_cell(this: Arc<Self>, mut cell: Cell)
        -> Box<Future<Item =CellHeader, Error = WriteError>>
    {
        box this.clone().pool.spawn_fn(move ||
            match this.server.chunks.write_cell(&mut cell) {
                Ok(header) => Ok(header),
                Err(e) => Err(e)
            }
        )
    }
    fn update_cell(this: Arc<Self>, mut cell: Cell)
        -> Box<Future<Item =CellHeader, Error = WriteError>>
    {
        box this.clone().pool.spawn_fn(move ||
            match this.server.chunks.update_cell(&mut cell) {
                Ok(header) => Ok(header),
                Err(e) => Err(e)
            }
        )
    }
    fn remove_cell(this: Arc<Self>, key: Id)
        -> Box<Future<Item = (), Error = WriteError>>
    {
        box this.clone().pool.spawn_fn(move ||this.server.chunks.remove_cell(&key))
    }
}

dispatch_rpc_service_functions!(NebRPCService);

impl NebRPCService {
    pub fn new(server: &Arc<NebServer>) -> Arc<NebRPCService> {
        Arc::new(NebRPCService {
            inner: Arc::new(NebRPCServiceInner {
                server: server.clone(),
                pool: CpuPool::new(4 * num_cpus::get())
            })
        })
    }
}