use bifrost::utils::async_locks::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use bifrost::utils::fut_exec::wait;
use client::AsyncClient;
use core::borrow::BorrowMut;
use dovahkiin::types::custom_types::id::Id;
use dovahkiin::types::custom_types::map::Map;
use dovahkiin::types::type_id_of;
use dovahkiin::types::value::ToValue;
use futures::Future;
use index::btree::*;
use itertools::Itertools;
use owning_ref::{OwningHandle, OwningRef, RcRef};
use ram::cell::Cell;
use ram::schema::{Field, Schema};
use ram::types::*;
use std::cell::Ref;
use std::cell::RefCell;
use std::cell::RefMut;
use std::collections::HashMap;
use std::mem;
use std::ops::Deref;
use std::ops::DerefMut;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use utils::lru_cache::LRUCache;

pub type ExtNodeCacheMap = Mutex<LRUCache<Id, Arc<RwLock<ExtNode>>>>;
pub type ExtNodeCachedMut = RwLockWriteGuard<ExtNode>;
pub type ExtNodeCachedImmute = RwLockReadGuard<ExtNode>;

const PAGE_SCHEMA: &'static str = "NEB_BTREE_PAGE";
const KEYS_FIELD: &'static str = "keys";
const NEXT_FIELD: &'static str = "next";
const PREV_FIELD: &'static str = "prev";

lazy_static! {
    static ref KEYS_KEY_HASH: u64 = key_hash(KEYS_FIELD);
    static ref NEXT_PAGE_KEY_HASH: u64 = key_hash(NEXT_FIELD);
    static ref PREV_PAGE_KEY_HASH: u64 = key_hash(PREV_FIELD);
    static ref PAGE_SCHEMA_ID: u32 = key_hash(PAGE_SCHEMA) as u32;
}

#[derive(Clone)]
pub struct ExtNode {
    pub id: Id,
    pub keys: EntryKeySlice,
    pub next: NodeCellRef,
    pub prev: NodeCellRef,
    pub len: usize,
    pub dirty: bool,
    pub cc: AtomicUsize
}

pub struct ExtNodeSplit {
    pub node_2: ExtNode,
    pub keys_1_len: usize,
}

impl ExtNode {
    pub fn new(id: Id) -> ExtNode {
        ExtNode {
            id: *id,
            keys: EntryKeySlice::init(),
            next: Node::none_ref(),
            prev: Node::none_ref(),
            len: 0,
            dirty: false,
            cc: AtomicUsize::new(0)
        }
    }
    pub fn from_cell(cell: Cell) -> Self {
        let cell_id = cell.id();
        let cell_version = cell.header.version;
        let next = cell.data[*NEXT_PAGE_KEY_HASH].Id().unwrap();
        let prev = cell.data[*PREV_PAGE_KEY_HASH].Id().unwrap();
        let keys = &cell.data[*KEYS_KEY_HASH];
        let keys_len = keys.len().unwrap();
        let keys_array = if let Value::PrimArray(PrimitiveArray::SmallBytes(ref array)) = keys {
            array
        } else {
            panic!()
        };
        let mut key_slice = EntryKeySlice::init();
        let mut key_count = 0;
        for (i, key_val) in keys_array.iter().enumerate() {
            key_slice[i] = EntryKey::from(key_val.as_slice());
            key_count += 1;
        }
        ExtNode {
            id: cell_id,
            keys: key_slice,
            next: *next,
            prev: *prev,
            len: key_count,
            dirty: false,
            cc: AtomicUsize::new(0),
        }
    }
    pub fn to_cell(&self) -> Cell {
        let mut value = Value::Map(Map::new());
        value[*NEXT_PAGE_KEY_HASH] = Value::Id(*self.next.get().ext_id());
        value[*PREV_PAGE_KEY_HASH] = Value::Id(*self.prev.get().ext_id());
        value[*KEYS_KEY_HASH] = self.keys[..self.len]
            .iter()
            .map(|key| SmallBytes::from_vec(key.as_slice().to_vec()))
            .collect_vec()
            .value();
        Cell::new_with_id(*PAGE_SCHEMA_ID, &self.id, value)
    }
    pub fn remove_at(&mut self, pos: usize) {
        let mut cached_len = self.len;
        debug!("Removing from external pos {}, len {}", pos, cached_len);
        self.keys.remove_at(pos, &mut cached_len);
        self.len = cached_len;
    }
    pub fn insert(
        &mut self,
        key: EntryKey,
        pos: usize,
        tree: &BPlusTree,
        self_ref: NodeCellRef,
    ) -> Option<(Node, Option<EntryKey>)> {
        let cached_len = cached.len;
        debug_assert!(cached_len <= NUM_KEYS);
        if cached_len == NUM_KEYS {
            // need to split
            debug!("insert to external with split, key {:?}, pos {}", &key, pos);
            // cached.dump();
            let pivot = cached_len / 2;
            let cached_next = &cached.next;
            let new_page_id = tree.new_page_id();
            let mut keys_1 = &mut cached.keys;
            let mut keys_2 = keys_1.split_at_pivot(pivot, cached_len);
            let mut keys_1_len = pivot;
            let mut keys_2_len = cached_len - pivot;
            // modify next node point previous to new node
            if !cached_next.is_unit_id() {
                let mut prev_node = bz.get_for_mut(&cached_next);
                prev_node.prev = new_page_id;
            }
            insert_into_split(
                key,
                keys_1,
                &mut keys_2,
                &mut keys_1_len,
                &mut keys_2_len,
                pos,
            );
            let extnode_2 = ExtNode {
                id: new_page_id,
                keys: keys_2,
                next: cached_next,
                prev: self_ref,
                len: keys_2_len,
                dirty: true,
                cc: AtomicUsize::new(0)
            };
            debug!(
                "new node have next {:?} prev {:?}, current id {:?}",
                extnode_2.next, extnode_2.prev, cached.id
            );
            cached.next = new_page_id;
            cached.len = keys_1_len;
            let node_2 = Node::External(box new_page_id);
            debug!(
                "Split to left len {}, right len {}",
                cached.len, extnode_2.len
            );
            bz.new_node(extnode_2);
            return Some((node_2, None));
        } else {
            debug!("insert to external without split at {}, key {:?}", pos, key);
            let mut new_cached_len = cached_len;
            cached.keys.insert_at(key, pos, &mut new_cached_len);
            cached.len = new_cached_len;
            return None;
        }
    }
    pub fn merge_with(&mut self, right: &mut Self) {
        debug!(
            "Merge external node, left len {}, right len {}",
            self.len, right.len
        );
        let self_len = self.len;
        let new_len = self.len + right.len;
        debug_assert!(new_len <= self.keys.len());
        for i in self.len..new_len {
            self.keys[i] = mem::replace(&mut right.keys[i - self_len], Default::default());
        }
        self.len = new_len;
    }
    pub fn dump(&self) {
        debug!("Dumping {:?}, keys {}", self.id, self.len);
        for i in 0..NUM_KEYS {
            debug!("{}\t- {:?}", i, self.keys[i]);
        }
    }
    pub fn remove_node(&self) {
        let id = &self.id;
        let mut prev = self.prev.get();
        let mut next = self.next.get();
        debug_assert_ne!(id, &Id::unit_id());
        if !prev.is_none() {
            let mut prev_node = prev.extnode_mut();
            prev_node.next = self.next.clone();
        }
        if !next.is_none() {
            let mut next_node = next.extnode_mut();
            next_node.prev = self.prev.clone();
        }
    }
}

pub fn rearrange_empty_extnode(node: &ExtNode) -> Id {
    let mut prev = node.prev.get();
    let mut next = node.next.get();
    if !prev.is_none() {
        let mut prev_node = prev.extnode_mut();
        prev_node.next = node.next.clone();
    }
    if !next.is_none() {
        let mut next_node = next.extnode_mut();
        next_node.prev = node.prev.clone();
    }
    return node.id;
}

pub fn page_schema() -> Schema {
    Schema {
        id: *PAGE_SCHEMA_ID,
        name: String::from(PAGE_SCHEMA),
        key_field: None,
        str_key_field: None,
        is_dynamic: false,
        fields: Field::new(
            "*",
            0,
            false,
            false,
            Some(vec![
                Field::new(NEXT_FIELD, type_id_of(Type::Id), false, false, None),
                Field::new(PREV_FIELD, type_id_of(Type::Id), false, false, None),
                Field::new(KEYS_FIELD, type_id_of(Type::SmallBytes), false, true, None),
            ]),
        ),
    }
}
