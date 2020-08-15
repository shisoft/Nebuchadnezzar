use dovahkiin::types::custom_types::id::Id;
use crate::index;
use crate::index::lsmtree::cursor::LSMTreeCursor;
use crate::index::lsmtree::placement::sm::client::SMClient;
use crate::index::lsmtree::tree::LSMTree;
use crate::index::lsmtree::tree::{KeyRange, LSMTreeResult};
use crate::index::trees::Cursor;
use crate::index::trees::EntryKey;
use linked_hash_map::LinkedHashMap;
use parking_lot::Mutex;
use parking_lot::MutexGuard;
use crate::ram::clock;
use crate::server::NebServer;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;

const CURSOR_DEFAULT_TTL: u32 = 5 * 60 * 1000;

struct DelegatedCursor {
    cursor: MutCursorRef,
    timestamp: u32,
}

type CursorMap = LinkedHashMap<u64, DelegatedCursor>;
type MutCursorRef = Rc<RefCell<LSMTreeCursor>>;

pub struct LSMTreeIns {
    pub tree: LSMTree,
    counter: AtomicU64,
    cursors: Mutex<CursorMap>,
}

impl DelegatedCursor {
    fn new(cursor: LSMTreeCursor) -> Self {
        let cursor = Rc::new(RefCell::new(cursor));
        let timestamp = clock::now();
        Self { cursor, timestamp }
    }
}

impl LSMTreeIns {
    pub fn new(range: KeyRange, id: Id) -> Self {
        Self::new_from_tree(LSMTree::new(range, id))
    }

    pub fn new_from_tree(tree: LSMTree) -> Self {
        Self {
            tree,
            counter: AtomicU64::new(0),
            cursors: Mutex::new(CursorMap::new()),
        }
    }

    fn get(&self, id: &u64) -> Option<MutCursorRef> {
        self.cursors.lock().get_refresh(id).map(|c| {
            c.timestamp = clock::now();
            c.cursor.clone()
        })
    }

    fn pop_expired_cursors(map: &mut MutexGuard<CursorMap>) {
        let mut expired_cursors = 0;
        while let Some((_, c)) = map.iter().next() {
            if c.timestamp + CURSOR_DEFAULT_TTL < clock::now() {
                expired_cursors += 1;
            } else {
                break;
            }
        }
        for _ in 0..expired_cursors {
            map.pop_front();
        }
    }

    pub fn seek(&self, key: &EntryKey, ordering: index::trees::Ordering) -> u64 {
        let cursor = self.tree.seek(key, ordering);
        let mut map = self.cursors.lock();
        Self::pop_expired_cursors(&mut map);
        let id = self.counter.fetch_and(1, Ordering::Relaxed);
        map.insert(id, DelegatedCursor::new(cursor));
        return id;
    }

    // Fetch th
    pub fn next_block(&self, id: &u64, block_size: usize) -> Option<Vec<Vec<u8>>> {
        self.get(id).map(|c| {
            let mut keys = Vec::with_capacity(block_size);
            let mut cursor = c.borrow_mut();
            let current = |cursor: &LSMTreeCursor| cursor.current().map(|k| k.as_slice().to_vec());
            if let Some(first_key) = current(&*cursor) {
                keys.push(first_key);
                while cursor.next() && keys.len() < block_size {
                    keys.push(current(&*cursor).unwrap());
                }
            }
            keys
        })
    }

    pub fn current(&self, id: &u64) -> Option<Option<Vec<u8>>> {
        self.get(id)
            .map(|c| c.borrow().current().map(|k| k.as_slice().to_vec()))
    }

    pub fn complete(&self, id: &u64) -> bool {
        self.cursors.lock().remove(id).is_some()
    }

    pub fn count(&self) -> u64 {
        self.tree.count() as u64
    }

    pub fn range(&self) -> (Vec<u8>, Vec<u8>) {
        let range = self.tree.range.lock();
        (range.0.clone().into_vec(), range.1.clone().into_vec())
    }

    pub fn check_and_merge(&self) {
        self.tree.check_and_merge()
    }

    pub fn oversized(&self) -> bool {
        self.tree.oversized()
    }

    pub fn insert(&self, key: EntryKey) -> bool {
        self.tree.insert(key)
    }

    pub fn with_epoch_check<F, T>(&self, epoch: u64, f: F) -> LSMTreeResult<T>
    where
        F: FnOnce() -> T,
    {
        let tree_epoch = self.tree.epoch();
        if tree_epoch != epoch {
            LSMTreeResult::EpochMismatch(tree_epoch, epoch)
        } else {
            LSMTreeResult::Ok(f())
        }
    }

    pub fn epoch(&self) -> u64 {
        self.tree.epoch()
    }

    pub fn merge(&self, keys: Box<Vec<EntryKey>>) {
        self.tree.merge(keys)
    }

    #[allow(dead_code)]
    pub fn remove_to_right(&self, start_key: &EntryKey) {
        self.tree.remove_to_right(start_key);
    }

    pub fn set_epoch(&self, epoch: u64) {
        self.tree.set_epoch(epoch);
    }

    #[allow(dead_code)]
    pub fn check_and_split(&self, _sm: &Arc<SMClient>, _neb: &Arc<NebServer>) -> Option<usize> {
        // self.tree.check_and_split(&self.tree, sm, neb)
        unimplemented!();
    }
}

unsafe impl Send for LSMTreeIns {}
unsafe impl Sync for LSMTreeIns {}
