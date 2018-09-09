use utils::lru_cache::LRUCache;
use dovahkiin::types::custom_types::id::Id;
use dovahkiin::types::custom_types::map::Map;
use std::cell::RefCell;
use ram::cell::Cell;
use std::cell::Ref;
use std::cell::RefMut;
use index::btree::*;
use bifrost::utils::async_locks::{RwLock, RwLockWriteGuard, RwLockReadGuard, Mutex};
use std::ops::Deref;
use std::ops::DerefMut;
use core::borrow::BorrowMut;
use dovahkiin::types::value::ToValue;
use itertools::Itertools;
use std::collections::BTreeMap;

pub type ExtNodeCacheMap = Mutex<LRUCache<Id, RwLock<ExtNode>>>;
pub type ExtNodeCachedMut = RwLockWriteGuard<ExtNode>;
pub type ExtNodeCachedImmute = RwLockReadGuard<ExtNode>;

lazy_static! {
    static ref KEYS_KEY_HASH : u64 = key_hash("keys");
    static ref NEXT_PAGE_KEY_HASH : u64 = key_hash("next");
    static ref PREV_PAGE_KEY_HASH : u64 = key_hash("prev");
    static ref PAGE_SCHEMA_ID: u32 = key_hash("BTREE_SCHEMA_ID") as u32;
}

#[derive(Clone)]
pub struct ExtNode {
    pub id: Id,
    pub keys: EntryKeySlice,
    pub next: Id,
    pub prev: Id,
    pub len: usize,
    pub version: u64,
    pub removed: bool,
}

pub struct ExtNodeSplit {
    pub node_2: ExtNode,
    pub keys_1_len: usize
}

impl ExtNode {
    pub fn new(id: &Id) -> ExtNode {
        ExtNode {
            id: *id,
            keys: EntryKeySlice::init(),
            next: Id::unit_id(),
            prev: Id::unit_id(),
            len: 0,
            version: 0,
            removed: false
        }
    }
    pub fn from_cell(cell: Cell) -> Self {
        let keys = &cell.data[*KEYS_KEY_HASH];
        let keys_len = keys.len().unwrap();
        let mut key_slice = EntryKeySlice::init();
        let mut key_count = 0;
        let next = cell.data[*NEXT_PAGE_KEY_HASH].Id().unwrap();
        let prev = cell.data[*PREV_PAGE_KEY_HASH].Id().unwrap();
        for (i, key_val) in keys.iter_value().unwrap().enumerate() {
            let key = if let Value::PrimArray(PrimitiveArray::U8(ref array)) = key_val {
                EntryKey::from_slice(array.as_slice())
            } else { panic!("invalid entry") };
            key_slice[i] = key;
            key_count += 1;
        }
        ExtNode {
            id: cell.id(),
            keys: key_slice,
            next: *next,
            prev: *prev,
            len: key_count,
            version: cell.header.version,
            removed: false
        }
    }
    pub fn to_cell(&self) -> Cell {
        let mut value = Value::Map(Map::new());
        value[*NEXT_PAGE_KEY_HASH] = Value::Id(self.next);
        value[*KEYS_KEY_HASH] = self
            .keys[..self.len]
            .iter()
            .map(|key| {
                key.as_slice().to_vec().value()
            })
            .collect_vec()
            .value();
        Cell::new_with_id(*PAGE_SCHEMA_ID, &self.id, value)
    }
    pub fn remove(&mut self, pos: usize) {
        let cached_len = self.len;
        self.keys.remove_at(pos, cached_len);
        self.len -= 1;
    }
    pub fn insert(&mut self, key: EntryKey, pos: usize, tree: &BPlusTree) -> Option<(Node, Option<EntryKey>)> {
        let mut cached = self;
        let cached_len = cached.len;
        if cached_len + 1 >= NUM_KEYS {
            // need to split
            let pivot = cached_len / 2;
            let split = {
                let cached_next = *&cached.next;
                let cached_id = *&cached.id;
                let new_page_id = tree.new_page_id();
                cached.next = new_page_id;
                let mut keys_1 = &mut cached.keys;
                let mut keys_2 = keys_1.split_at_pivot(pivot, cached_len);
                let mut keys_1_len = pivot;
                let mut keys_2_len = cached_len - pivot;
                insert_into_split(
                    key,
                    keys_1, &mut keys_2,
                    &mut keys_1_len, &mut keys_2_len,
                    pos, pivot);
                ExtNodeSplit {
                    keys_1_len,
                    node_2: ExtNode {
                        id: new_page_id,
                        keys: keys_2,
                        next: cached_next,
                        prev: cached_id,
                        len: keys_2_len,
                        version: 0,
                        removed: false
                    }
                }
            };
            cached.next = split.node_2.id;
            cached.len = split.keys_1_len;
            return Some((Node::External(box ExtNode::from_cached(split.node_2)), None));

        } else {
            cached.keys.insert_at(key, pos, cached_len);
            return None;
        }
    }
    pub fn update(&self) {

    }
}

pub fn rearrange_empty_extnode(sub_level_pointer: &Node, bz: &mut CacheBufferZone) -> Id {
    let node = sub_level_pointer.extnode(bz);
    let prev = node.prev;
    let next = node.next;
    if !prev.is_unit_id() {
        let mut prev_node = bz.get_for_mut(&prev);
        prev_node.next = next;
    }
    if !next.is_unit_id() {
        let mut next_node = bz.get_for_mut(&next);
        next_node.prev = prev;
    }
    return node.id;
}

#[derive(Clone)]
pub enum CacheGuardHolder {
    Read(ExtNodeCachedImmute),
    Write(ExtNodeCachedMut)
}

impl Deref for CacheGuardHolder {
    type Target = ExtNode;

    fn deref(&self) -> &'_ <Self as Deref>::Target {
        match self {
            &CacheGuardHolder::Read(l) => &*l,
            &CacheGuardHolder::Write(l) => &*l,
        }
    }
}

impl DerefMut for CacheGuardHolder {
    fn deref_mut(&mut self) -> &'_ mut <Self as Deref>::Target {
        match self {
            &mut CacheGuardHolder::Write(l) => &mut *l,
            _ => panic!("")
        }
    }
}

pub struct CacheBufferZone<'a> {
    tree: &'a BPlusTree,
    guards: BTreeMap<Id, CacheGuardHolder>,
    changes: BTreeMap<Id, Option<ExtNode>>
}


impl <'a> CacheBufferZone <'a> {
    pub fn new(tree: &BPlusTree) -> CacheBufferZone {
        CacheBufferZone {
            tree,
            guards: BTreeMap::new(),
            changes: BTreeMap::new()
        }
    }

    pub fn get(&mut self, id: &Id) -> &ExtNode {
        match self.changes.get(id) {
            Some(Some(changed)) => return &*changed,
            Some(None) => panic!(),
            _ => {}
        }
        if let Some(guard) = self.guards.get(id) {
            &*guard
        } else {
            let guard = self.tree.get_ext_node_cached(id);
            let holder = CacheGuardHolder::Read(guard);
            self.guards.insert(*id, holder);
            self.get(id)
        }
    }

    pub fn get_for_mut(&mut self, id: &Id) -> &mut ExtNode {
        match self.changes.get_mut(id) {
            Some(Some(ref mut changed)) => return &mut *changed,
            Some(None) => panic!(),
            _ => {}
        }
        if let Some(guard) = self.guards.get_mut(id) {
            unreachable!()
        } else {
            let guard = self.tree.get_mut_ext_node_cached(id);
            let holder = CacheGuardHolder::Write(guard);
            self.changes.insert(*id, Some((*holder).clone()));
            self.guards.insert(*id, holder);
            self.get_for_mut(id)
        }
    }

    pub fn set(&mut self, id: &Id, data: ExtNode) {
        self.changes.insert(*id, Some(data));
    }
    pub fn delete(&mut self, id: &Id) {
        self.changes.insert(*id, None);
    }

    pub fn flush(mut self) {
        for (id, data) in self.changes {
            if let Some(node) = data {
                let mut holder = self.guards.get_mut(&id).unwrap();
                if let &mut CacheGuardHolder::Write(mut guard) = holder {
                    *guard = node
                } else { panic!() }
            } else {
                unimplemented!();
            }
        }
    }
}