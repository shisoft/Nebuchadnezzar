use client::AsyncClient;
use index::btree::LevelTree;
use index::btree::NodeCellRef;
use index::btree::{BPlusTree, RTCursor as BPlusTreeCursor};
use index::key_with_id;
use index::lsmtree::cursor::LSMTreeCursor;
use index::Cursor;
use index::EntryKey;
use index::Ordering;
use index::*;
use itertools::Itertools;
use parking_lot::RwLock;
use ram::segs::MAX_SEGMENT_SIZE;
use ram::types::Id;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::{mem, ptr};

pub const LEVEL_ELEMENTS_MULTIPLIER: usize = 10;
pub const LEVEL_PAGE_DIFF_MULTIPLIER: usize = 10;

const LEVEL_M_MAX_ELEMENTS_COUNT: usize = LEVEL_M * LEVEL_M * LEVEL_M;
const LEVEL_M: usize = 24;
const LEVEL_1: usize = LEVEL_M * LEVEL_PAGE_DIFF_MULTIPLIER;
const LEVEL_2: usize = LEVEL_1 * LEVEL_PAGE_DIFF_MULTIPLIER;
const LEVEL_3: usize = LEVEL_2 * LEVEL_PAGE_DIFF_MULTIPLIER;
const LEVEL_4: usize = LEVEL_3 * LEVEL_PAGE_DIFF_MULTIPLIER;

type LevelTrees = Vec<Box<LevelTree>>;
pub type Ptr = NodeCellRef;
pub type Key = EntryKey;

with_levels! {
    LM, LEVEL_M;
    L1, LEVEL_1;
    L2, LEVEL_2;
    L3, LEVEL_3;
    // L4, LEVEL_4; // See https://github.com/rust-lang/rust/issues/58164
}

pub struct LSMTree {
    pub trees: LevelTrees,
    // use Vec here for convenience
    max_sizes: Vec<usize>,
}

unsafe impl Send for LSMTree {}
unsafe impl Sync for LSMTree {}

impl LSMTree {
    pub fn new(neb_client: &Arc<AsyncClient>) -> Self {
        debug!("Initializing LSM-tree...");
        let (trees, max_sizes) = init_lsm_level_trees(neb_client);
        debug!("Initialized LSM-tree");
        LSMTree { trees, max_sizes }
    }

    pub fn insert(&self, mut key: EntryKey, id: &Id) {
        key_with_id(&mut key, id);
        self.trees[0].insert_into(&key)
    }

    pub fn remove(&self, mut key: EntryKey, id: &Id) -> bool {
        key_with_id(&mut key, id);
        self.trees
            .iter()
            .map(|tree| tree.mark_key_deleted(&key))
            .collect_vec() // collect here to prevent short circuit
            .into_iter()
            .any(|d| d)
    }

    pub fn seek(&self, mut key: EntryKey, ordering: Ordering) -> LSMTreeCursor {
        match ordering {
            Ordering::Forward => key_with_id(&mut key, &Id::unit_id()),
            Ordering::Backward => key_with_id(&mut key, &Id::new(::std::u64::MAX, ::std::u64::MAX)),
        };
        let mut cursors: Vec<Box<Cursor>> = vec![];
        for tree in &self.trees {
            cursors.push(tree.seek_for(&key, ordering));
        }
        return LSMTreeCursor::new(cursors);
    }

    pub fn check_and_merge(&self) {
        for i in 0..self.trees.len() - 1 {
            debug!("Checking tree merge {}", i);
            let lower = &*self.trees[i];
            let upper = &*self.trees[i + 1];
            if lower.count() > self.max_sizes[i] {
                lower.merge_to(upper);
            }
        }
    }

    pub fn start_sentinel(this: &Arc<Self>) {
        let this = this.clone();
        thread::Builder::new()
            .name("LSM-Tree Sentinel".to_string())
            .spawn(move || loop {
                this.check_and_merge();
                thread::sleep(Duration::from_millis(500));
            });
    }

    pub fn level_sizes(&self) -> Vec<usize> {
        self.trees.iter().map(|t| t.count()).collect()
    }

    pub fn count(&self) -> usize {
        self.trees.iter().map(|t| t.count()).sum()
    }

    pub fn len(&self) -> usize {
        self.trees.iter().map(|tree| tree.count()).sum::<usize>()
    }
}