use super::btree::{BPlusTree, RTCursor as BPlusTreeCursor};
use super::*;
use client::AsyncClient;
use index::btree::LevelTree;
use index::btree::NodeCellRef;
use itertools::Itertools;
use parking_lot::RwLock;
use ram::segs::MAX_SEGMENT_SIZE;
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
    trees: LevelTrees,
    // use Vec here for convenience
    max_sizes: Vec<usize>,
}

unsafe impl Send for LSMTree {}
unsafe impl Sync for LSMTree {}

impl LSMTree {
    pub fn new(neb_client: &Arc<AsyncClient>) -> Arc<Self> {
        debug!("Initializing LSM-tree...");
        let (trees, max_sizes) = init_lsm_level_trees(neb_client);
        debug!("Initialized LSM-tree");
        let lsm_tree = Arc::new(LSMTree { trees, max_sizes });
        let tree_clone = lsm_tree.clone();
        thread::Builder::new().name("LSM-Tree Sentinel".to_string()).spawn(move || {
            tree_clone.sentinel();
        });
        lsm_tree
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

    fn sentinel(&self) {
        loop {
            for i in 0..self.trees.len() - 1 {
                debug!("Checking tree merge {}", i);
                let lower = &*self.trees[i];
                let upper = &*self.trees[i + 1];
                if lower.count() > self.max_sizes[i] {
                    lower.merge_to(upper);
                }
            }
            thread::sleep(Duration::from_millis(1000));
        }
    }

    pub fn level_sizes(&self) -> Vec<usize> {
        self.trees.iter().map(|t| t.count()).collect()
    }

    pub fn len(&self) -> usize {
        self.trees.iter().map(|tree| tree.count()).sum::<usize>()
    }
}

pub struct LSMTreeCursor {
    level_cursors: Vec<Box<Cursor>>,
}

impl LSMTreeCursor {
    fn new(cursors: Vec<Box<Cursor>>) -> Self {
        LSMTreeCursor {
            level_cursors: cursors,
        }
    }
}

impl Cursor for LSMTreeCursor {
    fn next(&mut self) -> bool {
        self.level_cursors
            .iter_mut()
            .filter(|c| c.current().is_some())
            .min_by(|a, b| a.current().unwrap().cmp(b.current().unwrap()))
            .map(|c| c.next())
            .unwrap_or(false)
    }

    fn current(&self) -> Option<&EntryKey> {
        self.level_cursors.iter().filter_map(|c| c.current()).min()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use byteorder::BigEndian;
    use byteorder::WriteBytesExt;
    use client;
    use futures::prelude::*;
    use rand::distributions::Uniform;
    use rand::thread_rng;
    use rand::Rng;
    use rayon::prelude::*;
    use server::NebServer;
    use server::ServerOptions;
    use std::env;
    use std::io::Cursor as StdCursor;

    fn u64_to_slice(n: u64) -> [u8; 8] {
        let mut key_slice = [0u8; 8];
        {
            let mut cursor = StdCursor::new(&mut key_slice[..]);
            cursor.write_u64::<BigEndian>(n);
        };
        key_slice
    }

    #[test]
    pub fn insertions() {
        env_logger::init();
        let server_group = "sstable_index_init";
        let server_addr = String::from("127.0.0.1:5700");
        let server = NebServer::new_from_opts(
            &ServerOptions {
                chunk_count: 1,
                memory_size: 3 * 1024 * 1024 * 1024,
                backup_storage: None,
                wal_storage: None,
            },
            &server_addr,
            &server_group,
        );
        let client = Arc::new(
            client::AsyncClient::new(&server.rpc, &vec![server_addr], server_group).unwrap(),
        );
        client.new_schema_with_id(btree::page_schema()).wait();
        let tree = LSMTree::new(&client);
        let num = env::var("LSM_TREE_TEST_ITEMS")
            // this value cannot do anything useful to the test
            // must arrange a long-term test to cover every levels
            .unwrap_or("1000".to_string())
            .parse::<u64>()
            .unwrap();

        let tree_clone = tree.clone();
        debug!("Testing LSM-tree");
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(10));
            let tree_len = tree_clone.len();
            debug!(
                "LSM-Tree now have {}/{} elements, total {:.2}%",
                tree_len,
                num,
                tree_len as f32 / num as f32 * 100.0
            );
        });

        (0..num).collect::<Vec<_>>().par_iter().for_each(|i| {
            let i = *i;
            let id = Id::new(0, i);
            let key_slice = u64_to_slice(i);
            let key = SmallVec::from_slice(&key_slice);
            tree.insert(key, &id);
        });

        if tree.len() > LEVEL_M_MAX_ELEMENTS_COUNT {
            debug!("Level trees sizes are {:?}, wait for continuous merge", tree.level_sizes());
            thread::sleep(Duration::from_secs(5));
            debug!("Level trees sizes are {:?}", tree.level_sizes());
        }
        debug!("Start validations");
        (0..num).collect::<Vec<_>>().iter().for_each(|i| {
            let mut rng = rand::rngs::OsRng::new().unwrap();
            let die_range = Uniform::new_inclusive(1, 6);
            let mut roll_die = rng.sample_iter(&die_range);
            let i = *i;
            let id = Id::new(0, i);
            let key_slice = u64_to_slice(i);
            let mut key = SmallVec::from_slice(&key_slice);
            debug!("checking: {}", i);
            let mut cursor = tree.seek(key.clone(), Ordering::Forward);
            key_with_id(&mut key, &id);
            assert_eq!(cursor.current(), Some(&key), "{}", i);
            if roll_die.next().unwrap() == 6 {
                for j in i..num {
                    let id = Id::new(0, j);
                    let key_slice = u64_to_slice(j);
                    let mut key = SmallVec::from_slice(&key_slice);
                    key_with_id(&mut key, &id);
                    assert_eq!(cursor.current(), Some(&key), "{}/{}", i, j);
                    assert_eq!(cursor.next(), j != num - 1, "{}/{}", i, j);
                }
            }
        });
    }
}
