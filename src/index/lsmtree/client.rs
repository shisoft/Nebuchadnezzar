use crate::index::lsmtree::placement::sm::client::SMClient as PlacementClient;
use crate::index::lsmtree::placement::sm::{Placement as PlacementMeta, QueryError};
use crate::index::lsmtree::service::{AsyncServiceClient, LSMTreeSvrError};
use crate::index::trees::{EntryKey, KEY_SIZE, Ordering};
use linked_hash_map::LinkedHashMap;
use parking_lot::{RwLock, Mutex};
use crate::ram::types::Id;
use crate::server::NebServer;
use std::collections::btree_map::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;
use futures::prelude::*;
use crate::index::lsmtree::split::tree_client;
use crate::index::lsmtree::placement;
use crate::index::builder::Feature;
use byteorder::{BigEndian, WriteBytesExt};
use crate::index::lsmtree::tree::LSMTreeResult;
use bifrost::conshash::ConsistentHashing;
use crate::index::lsmtree;
use bifrost::raft::client::RaftClient;

const SEEK_BLOCK_SIZE: u32 = 128;

pub struct Cursor {
    tree: SubTree,
    id: u64,
    buffer: Vec<Vec<u8>>,
    current: usize
}

pub struct Placement {
    meta: PlacementMeta,
    client: Arc<AsyncServiceClient>,
}

pub struct LSMTreeClient {
    counter: AtomicUsize,
    placements: RwLock<BTreeMap<Vec<u8>, Placement>>,
    cursors: RwLock<LinkedHashMap<usize, Mutex<Cursor>>>,
    placement_client: PlacementClient,
    neb: Arc<NebServer>,
}

pub struct SubTree {
    client: Arc<AsyncServiceClient>,
    epoch: u64,
    tree_id: Id,
    starts: Vec<u8>
}

impl SubTree {
    pub fn new(tree_id: Id, client: Arc<AsyncServiceClient>, epoch: u64, starts: &Vec<u8>) -> Self {
        Self {
            tree_id, client, epoch, starts: starts.clone()
        }
    }
}

impl LSMTreeClient {

    pub fn new(neb: &Arc<NebServer>, raft_client: &Arc<RaftClient>,) -> Self {
        Self {
            counter: AtomicUsize::new(0),
            placements: RwLock::new(BTreeMap::new()),
            cursors: RwLock::new(LinkedHashMap::new()),
            placement_client: PlacementClient::new(
                lsmtree::placement::sm::SM_ID,
                raft_client
            ),
            neb: neb.clone()
        }
    }

    async fn update_placement(&self, sub_tree: &SubTree) {
        match self.placement_client.get(&sub_tree.tree_id).await.unwrap() {
            Ok(placement) => {
                let mut placements = self.placements.write();
                let rpc_client = tree_client(&placement.id, &self.neb).await.unwrap();
                placements.remove(&sub_tree.starts);
                placements.insert(placement.starts.clone(), Placement {
                    meta: placement,
                    client: rpc_client
                });
            },
            Err(QueryError::PlacementNotFound) => panic!("cannot find a suitable placement"),
            Err(e) => panic!("{:?}", e)
        }
    }

    async fn get_sub_tree(&self, key: &Vec<u8>) -> SubTree {
        // Stage one, early exit if placement founded. Read lock only
        {
            let placements = self.placements.read();
            let candidate = placements.range::<Vec<u8>, _>(key..).next();
            if let Some((_, candidate_placement)) = candidate {
                if &candidate_placement.meta.ends >= key {
                    // in range, return
                    return SubTree::new(
                        candidate_placement.meta.id,
                        candidate_placement.client.clone(),
                        candidate_placement.meta.epoch,
                        &candidate_placement.meta.starts
                    );
                }
            }
        }

        // Stage two, write lock. Also check for placement availability. If not, insert it to the cache.
        {
            let mut placements = self.placements.write();
            let candidate = placements.range::<Vec<u8>, _>(key..).next();
            if let Some((_, candidate_placement)) = candidate {
                if &candidate_placement.meta.ends >= key {
                    // in range, return
                    return SubTree::new(
                        candidate_placement.meta.id,
                        candidate_placement.client.clone(),
                        candidate_placement.meta.epoch,
                        &candidate_placement.meta.starts
                    );
                }
            }
            let placement: PlacementMeta = self.placement_client.locate(&Vec::from(key.as_slice()))
                .await.unwrap().unwrap();
            let rpc_client = tree_client(&placement.id, &self.neb)
                .await.unwrap();
            let sub_tree = SubTree::new(
                placement.id,
                rpc_client.clone(),
                placement.epoch,
                &placement.starts
            );
            placements.insert(placement.starts.clone(), Placement {
                meta: placement,
                client: rpc_client
            });
            sub_tree
        }
    }

    fn essential_key_components(schema_id: u32, field_id: u64) -> Vec<u8> {
        let mut key = Vec::with_capacity(KEY_SIZE);
        let mut schema_id_fut = [0u8; 4];
        let mut field_id_fut = [0u8; 8];
        (&mut schema_id_fut as &mut [u8]).write_u32::<BigEndian>(schema_id).unwrap();
        (&mut field_id_fut as &mut [u8]).write_u64::<BigEndian>(field_id).unwrap();
        key.extend_from_slice(&schema_id_fut);  // 4 bytes
        key.extend_from_slice(&field_id_fut);   // 8 bytes
        key
    }

    pub async fn insert(&self,schema_id: u32, field_id: u64, cell_id: &Id, feature: &Feature) -> bool {
        let mut key = Self::essential_key_components(schema_id, field_id);
        key.extend_from_slice(feature); // 8 bytes
        key.extend_from_slice(&cell_id.to_binary()); // ID SIZE
        loop {
            let sub_tree = self.get_sub_tree(&key).await;
            let tree_client = &sub_tree.client;
            let insertion_result = tree_client
                    .insert(sub_tree.tree_id, key.clone(), sub_tree.epoch)
                    .await.unwrap();
            match insertion_result {
                Ok(LSMTreeResult::Ok(insert_res)) => {
                    return insert_res;
                },
                Ok(LSMTreeResult::EpochMismatch(_, _)) | Err(LSMTreeSvrError::TreeNotFound) => {
                    self.update_placement(&sub_tree);
                },
                Err(_) => {
                    panic!("Error occurred on distributed LSM-tree insertion");
                }
            }
        }
    }

    pub async fn seek(&self, schema_id: u32, field_id: u64, feature: &Feature, ordering: Ordering) -> Option<Cursor> {
        let mut key = Self::essential_key_components(schema_id, field_id);
        key.extend_from_slice(feature); // 8 bytes
        loop {
            let sub_tree = self.get_sub_tree(&key).await;
            let tree_client = &sub_tree.client;
            let seek_result = tree_client.seek(
                sub_tree.tree_id,
                key.clone(),
                ordering,
                sub_tree.epoch,
                SEEK_BLOCK_SIZE
            ).await.unwrap();
            match seek_result {
                Ok(LSMTreeResult::Ok(insert_res)) => {
                    return insert_res.map(|block| {
                        Cursor {
                            tree: sub_tree,
                            id: block.cursor_id,
                            buffer: block.data,
                            current: 0
                        }
                    })
                },
                Ok(LSMTreeResult::EpochMismatch(_, _)) | Err(LSMTreeSvrError::TreeNotFound) => {
                    self.update_placement(&sub_tree);
                },
                Err(_) => {
                    panic!("Error occurred on distributed LSM-tree seek");
                }
            }
        }
    }
}