use libc;
use std::sync::{Arc};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::collections::BTreeSet;
use parking_lot::{Mutex, RwLock, RwLockReadGuard};
use chashmap::{CHashMap, ReadGuard, WriteGuard};
use ram::schema::SchemasServer;
use ram::types::Id;
use ram::segs::{Segment, SEGMENT_SIZE};
use ram::cell::{Cell, ReadError, WriteError, Header};
use server::ServerMeta;

pub type CellReadGuard<'a> = ReadGuard<'a, u64, usize>;
pub type CellWriteGuard<'a> = WriteGuard<'a, u64, usize>;

pub struct Chunk {
    pub id: usize,
    pub addr: usize,
    pub index: CHashMap<u64, usize>,
    pub segs: Vec<Segment>,
    pub seg_round: AtomicUsize,
    pub meta: Arc<ServerMeta>,
    pub backup_storage: Option<String>,
}

pub struct Chunks {
    pub list: Vec<Chunk>,
}

impl Chunk {
    fn new (id: usize, size: usize, meta: Arc<ServerMeta>, back_storage: Option<String>) -> Chunk {
        let mem_ptr = unsafe {libc::malloc(size)} as usize;
        let seg_count = size / SEGMENT_SIZE;
        let mut segments = Vec::<Segment>::new();
        for seg_idx in 0..seg_count {
            let seg_addr = mem_ptr + seg_idx * SEGMENT_SIZE;
            segments.push(Segment {
                addr: seg_addr,
                id: seg_idx,
                bound: seg_addr + SEGMENT_SIZE,
                append_header: AtomicUsize::new(seg_addr),
                lock: RwLock::new(()),
                frags: Mutex::new(BTreeSet::new()),
            });
        }
        debug!("creating chunk at {}, segments {}", mem_ptr, seg_count + 1);
        Chunk {
            id: id,
            addr: mem_ptr,
            index: CHashMap::new(),
            meta: meta,
            segs: segments,
            seg_round: AtomicUsize::new(0),
            backup_storage: back_storage
        }
    }
    pub fn try_acquire(&self, size: usize) -> Option<(usize, RwLockReadGuard<()>)> {
        let mut retried = 0;
        loop {
            let n = self.seg_round.load(Ordering::Relaxed);
            let seg_id = n % self.segs.len();
            let seg_acquire = self.segs[seg_id].try_acquire(size);
            match seg_acquire {
                None => {
                    if retried > self.segs.len() * 2 {return None;}
                    self.seg_round.fetch_add(1, Ordering::Relaxed);
                    retried += 1;
                },
                _ => {return seg_acquire;}
            }
        }
    }
    fn locate_segment(&self, location: usize) -> &Segment {
        let offset = location - self.addr;
        let seg_id = offset / SEGMENT_SIZE;
        return &self.segs[seg_id];
    }
    pub fn location_for_read<'a>(&self, hash: u64)
        -> Option<CellReadGuard> {
        match self.index.get(&hash) {
            Some(index) => {
                if *index == 0 {
                    return None
                }
                return Some(index);
            },
            None => None
        }
    }
    pub fn location_for_write(&self, hash: u64)
        -> Option<CellWriteGuard> {
        match self.index.get_mut(&hash) {
            Some(index) => {
                if *index == 0 {
                    return None
                }
                return Some(index);
            },
            None => None
        }
    }
    fn put_tombstone(&self, location: usize) {
        let seg = self.locate_segment(location);
        seg.put_cell_tombstone(location);
        seg.put_frag(location);
    }
    fn head_cell(&self, hash: u64) -> Result<Header, ReadError> {
        match self.location_for_read(hash) {
            Some(loc) => {
                Cell::header_from_chunk_raw(*loc)
            },
            None => Err(ReadError::CellDoesNotExisted)
        }
    }
    fn read_cell(&self, hash: u64) -> Result<Cell, ReadError> {
        match self.location_for_read(hash) {
            Some(loc) => {
                Cell::from_chunk_raw(*loc, self)
            },
            None => Err(ReadError::CellDoesNotExisted)
        }
    }
    fn write_cell(&self, cell: &mut Cell) -> Result<Header, WriteError> {
        let hash = cell.header.hash;
        if self.location_for_read(hash).is_some() {
            return Err(WriteError::CellAlreadyExisted);
        } else {
            let loc = cell.write_to_chunk(self)?;
            let mut need_rollback = false;
            self.index.upsert(
                hash,
                ||{loc},
                |inserted_loc| {
                    if *inserted_loc == 0 {
                        *inserted_loc = loc
                    } else {
                        need_rollback = true;
                    }
                }
            );
            if need_rollback {
                self.put_tombstone(loc);
                return Err(WriteError::CellAlreadyExisted)
            }
            return Ok(cell.header)
        }
    }
    fn update_cell(&self, cell: &mut Cell) -> Result<Header, WriteError> {
        let hash = cell.header.hash;
        if let Some(mut cell_location) = self.location_for_write(hash) {
            let new_location = cell.write_to_chunk(self)?;
            let old_location = *cell_location;
            *cell_location = new_location;
            self.put_tombstone(old_location);
            return Ok(cell.header);
        } else {
            return Err(WriteError::CellDoesNotExisted)
        }
    }
    fn update_cell_by<U>(&self, hash: u64, update: U) -> Result<Cell, WriteError>
        where U: Fn(Cell) -> Option<Cell> {
        if let Some(mut cell_location) = self.location_for_write(hash) {
            let cell = Cell::from_chunk_raw(*cell_location, self);
            match cell {
                Ok(cell) => {
                    let mut new_cell = update(cell);
                    if let Some(mut new_cell) = new_cell {
                        let new_location = new_cell.write_to_chunk(self)?;
                        let old_location = *cell_location;
                        *cell_location = new_location;
                        self.put_tombstone(old_location);
                        return Ok(new_cell);
                    } else {
                        return Err(WriteError::UserCanceledUpdate);
                    }
                },
                Err(e) => Err(WriteError::ReadError(e))
            }
        } else {
            return Err(WriteError::CellDoesNotExisted)
        }
    }
    fn remove_cell(&self, hash: u64) -> Result<(), WriteError> {
        if let Some(cell_location) = self.index.remove(&hash) {
            self.put_tombstone(cell_location);
            Ok(())
        } else {
            Err(WriteError::CellDoesNotExisted)
        }
    }
    fn remove_cell_by<P>(&self, hash: u64, predict: P) -> Result<(), WriteError>
        where P: Fn(Cell) -> bool {
        let mut result = Ok(());
        self.index.alter(hash, |loc_opt|{
            match loc_opt {
                Some(cell_location) => {
                    let cell = Cell::from_chunk_raw(cell_location, self);
                    match cell {
                        Ok(cell) => {
                            if predict(cell) {
                                self.put_tombstone(cell_location);
                                None
                            } else {
                                result = Err(WriteError::CellDoesNotExisted);
                                loc_opt
                            }
                        },
                        Err(e) => {
                            result = Err(WriteError::ReadError(e));
                            None
                        }
                    }
                },
                None => {
                    result = Err(WriteError::CellDoesNotExisted);
                    None
                }
            }
        });
        return result;
    }
    fn dispose (&mut self) {
        debug!("disposing chunk at {}", self.addr);
        unsafe {
            libc::free(self.addr as *mut libc::c_void)
        }
    }
}

impl Drop for Chunk {
    fn drop(&mut self) {
        self.dispose();
    }
}

impl Chunks {
    pub fn new (count: usize, size: usize, meta: Arc<ServerMeta>, backup_storage: Option<String>) -> Arc<Chunks> {
        let chunk_size = size / count;
        let mut chunks = Vec::new();
        debug!("Creating {} chunks, total {} bytes", count, size);
        for i in 0..count {
            let backup_storage = match backup_storage {
                Some(ref dir) => Some(format!("{}/data-{}.bak", dir, i)),
                None => None
            };
            chunks.push(Chunk::new(i, chunk_size, meta.clone(), backup_storage));
        }
        Arc::new(Chunks {
            list: chunks
        })
    }
    pub fn new_dummy(count: usize, size: usize) -> Arc<Chunks> {
        Chunks::new(count, size, Arc::<ServerMeta>::new(ServerMeta {
            schemas: SchemasServer::new(None)
        }), None)
    }
    fn locate_chunk_by_partition(&self, partition: u64) -> &Chunk {
        let chunk_id = partition as usize % self.list.len();
        return &self.list[chunk_id];
    }
    fn locate_chunk_by_key(&self, key: &Id) -> (&Chunk, u64) {
        return (self.locate_chunk_by_partition(key.higher), key.lower);
    }
    pub fn read_cell(&self, key: &Id) -> Result<Cell, ReadError> {
        let (chunk, hash) = self.locate_chunk_by_key(key);
        return chunk.read_cell(hash);
    }
    pub fn head_cell(&self, key: &Id) -> Result<Header, ReadError> {
        let (chunk, hash) = self.locate_chunk_by_key(key);
        return chunk.head_cell(hash);
    }
    pub fn location_for_read(&self, key: &Id) -> Option<CellReadGuard> {
        let (chunk, hash) = self.locate_chunk_by_key(key);
        return chunk.location_for_read(hash);
    }
    pub fn write_cell(&self, cell: &mut Cell) -> Result<Header, WriteError> {
        let chunk = self.locate_chunk_by_partition(cell.header.partition);
        return chunk.write_cell(cell);
    }
    pub fn update_cell(&self, cell: &mut Cell) -> Result<Header, WriteError> {
        let chunk = self.locate_chunk_by_partition(cell.header.partition);
        return chunk.update_cell(cell);
    }
    pub fn update_cell_by<U>(&self, key: &Id, update: U) -> Result<Cell, WriteError>
        where U: Fn(Cell) -> Option<Cell>{
        let (chunk, hash) = self.locate_chunk_by_key(key);
        return chunk.update_cell_by(hash, update);
    }
    pub fn remove_cell(&self, key: &Id) -> Result<(), WriteError> {
        let (chunk, hash) = self.locate_chunk_by_key(key);
        return chunk.remove_cell(hash);
    }
    pub fn remove_cell_by<P>(&self, key: &Id, predict: P) -> Result<(), WriteError>
        where P: Fn(Cell) -> bool {
        let (chunk, hash) = self.locate_chunk_by_key(key);
        return chunk.remove_cell_by(hash, predict);
    }
    pub fn chunk_ptr(&self, key: &Id) -> usize {
        let (chunk, hash) = self.locate_chunk_by_key(key);
        return *chunk.location_for_read(hash).unwrap()
    }
}