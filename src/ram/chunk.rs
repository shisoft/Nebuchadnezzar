use libc;
use std::thread;
use std::rc::Rc;
use std::sync::{Arc};
use std::sync::atomic::{AtomicUsize, Ordering};
use parking_lot::{Mutex, MutexGuard, RwLock};
use concurrent_hashmap::ConcHashMap;
use ram::schema::Schemas;
use ram::segs::{Segment, SEGMENT_SIZE};
use ram::cell::{Cell, ReadError, WriteError};
use server::ServerMeta;

pub struct Chunk {
    pub id: usize,
    pub addr: usize,
    pub index: ConcHashMap<u64, Mutex<usize>>,
    pub segs: Vec<Segment>,
    pub seg_round: AtomicUsize,
    pub meta: Rc<ServerMeta>,
    pub backup_storage: Option<String>,
}

pub struct Chunks {
    pub list: Vec<Chunk>,
}

impl Chunk {
    fn new (id: usize, size: usize, meta: Rc<ServerMeta>, back_storage: Option<String>) -> Chunk {
        let mem_ptr = unsafe {libc::malloc(size)} as usize;
        let seg_count = size / SEGMENT_SIZE;
        let mut segments = Vec::<Segment>::new();
        for seg_idx in 0..seg_count {
            let seg_addr = mem_ptr + seg_idx * SEGMENT_SIZE;
            segments.push(Segment {
                addr: seg_addr,
                id: seg_idx,
                bound: seg_addr + SEGMENT_SIZE,
                last: AtomicUsize::new(seg_addr),
                lock: RwLock::new(()),
                tombstones: Vec::new(),
            });
        }
        debug!("creating chunk at {}, segments {}", mem_ptr, seg_count + 1);
        Chunk {
            id: id,
            addr: mem_ptr,
            index: ConcHashMap::<u64, Mutex<usize>>::new(),
            meta: meta,
            segs: segments,
            seg_round: AtomicUsize::new(0),
            backup_storage: back_storage
        }
    }
    pub fn try_acquire(&self, size: usize) -> Option<usize> {
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
    fn location_of(&self, hash: u64) -> Option<MutexGuard<usize>> {
        match self.index.find(&hash) {
            Some(index) => {
                let index = index.get();
                let index_lock = index.lock();
                if *index_lock == 0 {
                    return None
                }
                return Some(index_lock);
            },
            None => None
        }
    }
    fn read_cell(&self, hash: u64) -> Result<Cell, ReadError> {
        match self.location_of(hash) {
            Some(loc) => {
                Cell::from_chunk_raw(*loc, self)
            },
            None => Err(ReadError::CellDoesNotExisted)
        }
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
    pub fn new (count: usize, size: usize, meta: Rc<ServerMeta>, backup_storage: Option<String>) -> Chunks {
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
        Chunks {
            list: chunks
        }
    }

    pub fn new_dummy(count: usize, size: usize) -> Chunks {
        Chunks::new(count, size, Rc::<ServerMeta>::new(ServerMeta {
            schemas: Schemas::new(None)
        }), None)
    }
}