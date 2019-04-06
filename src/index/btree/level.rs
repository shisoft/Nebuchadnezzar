use index::btree::external::ExtNode;
use index::btree::internal::InNode;
use index::btree::node::is_node_locked;
use index::btree::node::read_node;
use index::btree::node::read_unchecked;
use index::btree::node::write_node;
use index::btree::node::write_non_empty;
use index::btree::node::write_targeted;
use index::btree::node::write_unchecked;
use index::btree::node::EmptyNode;
use index::btree::node::Node;
use index::btree::node::NodeData;
use index::btree::node::NodeWriteGuard;
use index::btree::search::mut_search;
use index::btree::search::MutSearchResult;
use index::btree::LevelTree;
use index::btree::NodeCellRef;
use index::btree::{external, BPlusTree};
use index::lsmtree::tree::LEVEL_PAGE_DIFF_MULTIPLIER;
use index::EntryKey;
use index::Slice;
use itertools::Itertools;
use smallvec::SmallVec;
use std::collections::BTreeSet;
use std::fmt::Debug;
use std::mem;
use std::sync::atomic::Ordering::Relaxed;

enum Selection<KS, PS>
where
    KS: Slice<EntryKey> + Debug + 'static,
    PS: Slice<NodeCellRef> + 'static,
{
    Selected(Vec<NodeWriteGuard<KS, PS>>),
    Innode(NodeCellRef),
}

enum PruningSearch {
    DeepestInnode,
    Innode(NodeCellRef),
}

struct NodeRemoval<'a> {
    empty_pages: Vec<&'a EntryKey>,
    split: Vec<(NodeCellRef, EntryKey)>,
}

impl<'a> NodeRemoval<'a> {
    fn new() -> Self {
        Self {
            empty_pages: vec![],
            split: vec![],
        }
    }
}

fn select<KS, PS>(node: &NodeCellRef) -> Vec<NodeWriteGuard<KS, PS>>
where
    KS: Slice<EntryKey> + Debug + 'static,
    PS: Slice<NodeCellRef> + 'static,
{
    let search = mut_search::<KS, PS>(node, &smallvec!());
    match search {
        MutSearchResult::External => {
            let mut collected = vec![write_node(node)];
            while collected.len() < LEVEL_PAGE_DIFF_MULTIPLIER {
                let right = write_node(
                    collected
                        .last_mut()
                        .unwrap()
                        .right_ref_mut_no_empty()
                        .unwrap(),
                );
                if right.is_none() {
                    break;
                } else {
                    collected.push(right);
                }
            }
            return collected;
        }
        MutSearchResult::Internal(node) => select::<KS, PS>(&node),
    }
}

fn merge_innode_remnant<'a, KS, PS>(
    current_node: &mut NodeWriteGuard<KS, PS>,
    prev_key: &'a EntryKey,
    removal: &mut NodeRemoval<'a>,
) where
    KS: Slice<EntryKey> + Debug + 'static,
    PS: Slice<NodeCellRef> + 'static,
{
    let curr_right_ref = current_node.right_ref_mut_no_empty().unwrap().clone();
    let curr_innode = current_node.innode_mut();
    let curr_right_bound = &curr_innode.right_bound;
    let curr_last_child = mem::replace(&mut curr_innode.ptrs.as_slice()[0], NodeCellRef::default());
    debug_assert_eq!(curr_innode.len, 0);
    removal.empty_pages.push(prev_key);
    if curr_last_child.is_default() {
        return;
    }
    let mut next_node = write_targeted(write_node::<KS, PS>(&curr_right_ref), curr_right_bound);
    let pos = next_node.search(curr_right_bound);
    let next_innode = next_node.innode_mut();
    next_innode.debug_check_integrity();
    if next_innode.len == KS::slice_len() {
        removal.split.push(next_innode.split_insert(
            curr_right_bound.clone(),
            curr_last_child,
            pos,
            false,
        ))
    } else {
        next_innode.insert_in_place(curr_right_bound.clone(), curr_last_child, pos, false);
    }
}

fn apply_removal<'a, KS, PS>(
    cursor_guard: &mut NodeWriteGuard<KS, PS>,
    poses: &mut BTreeSet<usize>,
    node_removal: &mut NodeRemoval<'a>,
    prev_key: &Option<&'a EntryKey>,
    remove_children_right_nodes: bool,
) where
    KS: Slice<EntryKey> + Debug + 'static,
    PS: Slice<NodeCellRef> + 'static,
{
    if poses.is_empty() {
        return;
    }
    debug!(
        "Applying removal, guard key {:?}, poses {:?}",
        cursor_guard.first_key(),
        poses
    );
    {
        let innode = cursor_guard.innode_mut();
        let mut new_keys = KS::init();
        let mut new_ptrs = PS::init();
        {
            if remove_children_right_nodes {
                // A node will be reclaimed if and only if it have zero references
                // If there is any sequential empty nodes, they have to be unlinked each other
                // The upper level will do the rest to unlink it from the tree
                innode.ptrs.as_slice()[..innode.len + 1]
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| poses.contains(i))
                    .for_each(|(_, r)| {
                        write_node::<KS, PS>(r).right_ref_mut_no_empty();
                    });
            }
            let ptrs: Vec<&mut _> = innode.ptrs.as_slice()[..innode.len + 1]
                .iter_mut()
                .enumerate()
                .filter(|(i, _)| !poses.contains(i))
                .map(|(_, p)| p)
                .collect();

            let keys: Vec<&mut _> = innode.keys.as_slice()[..innode.len]
                .iter_mut()
                .enumerate()
                .filter(|(i, _)| !poses.contains(i))
                .take(if ptrs.len() == 0 { 0 } else { ptrs.len() - 1 })
                .map(|(_, k)| k)
                .collect();
            innode.len = keys.len();
            debug!("Prune filtered page have keys {:?}", &keys);
            for (i, key) in keys.into_iter().enumerate() {
                debug_assert!(key > &mut smallvec!(0));
                mem::swap(&mut new_keys.as_slice()[i], key);
            }
            for (i, ptr) in ptrs.into_iter().enumerate() {
                debug_assert!(!ptr.is_default());
                mem::swap(&mut new_ptrs.as_slice()[i], ptr);
            }
        }
        innode.keys = new_keys;
        innode.ptrs = new_ptrs;
        innode.debug_check_integrity();

        debug!(
            "Pruned page have keys {:?}",
            &innode.keys.as_slice_immute()[..innode.len]
        );
    }

    if cursor_guard.is_empty() {
        if let &Some(k) = prev_key {
            debug_assert!(!cursor_guard.is_ext());
            merge_innode_remnant(cursor_guard, k, node_removal);
        }
        debug!("Pruned page is empty: {:?}", prev_key);
        cursor_guard.make_empty_node();
    }
    poses.clear();
}

fn prune_selected<'a, KS, PS>(
    node: &NodeCellRef,
    mut removal: Box<NodeRemoval<'a>>,
    level: usize,
) -> Box<NodeRemoval<'a>>
where
    KS: Slice<EntryKey> + Debug + 'static,
    PS: Slice<NodeCellRef> + 'static,
{
    let first_search = mut_search::<KS, PS>(node, removal.empty_pages.first().unwrap());
    let pruning = match first_search {
        MutSearchResult::Internal(sub_node) => {
            if read_unchecked::<KS, PS>(&NodeData::<KS, PS>::get_non_empty_node(&sub_node)).is_ext()
            {
                PruningSearch::DeepestInnode
            } else {
                PruningSearch::Innode(sub_node)
            }
        }
        MutSearchResult::External => unreachable!(),
    };
    let mut deepest = false;
    match pruning {
        PruningSearch::DeepestInnode => {
            debug!(
                "Removing in deepest nodes keys {:?}, level {}",
                &removal.empty_pages, level
            );
            deepest = true;
        }
        PruningSearch::Innode(sub_node) => {
            removal = prune_selected::<KS, PS>(&sub_node, removal, level + 1);
        }
    }
    // empty page references that will dealt with by upper level
    let mut upper_removal = NodeRemoval::new();
    if !removal.empty_pages.is_empty() {
        debug!(
            "Pruning page containing keys {:?}, level {}",
            &removal.empty_pages, level
        );
        // start delete
        let mut cursor_guard = write_non_empty(write_node::<KS, PS>(node));
        let mut guard_removing_poses = BTreeSet::new();
        let mut prev_key = None;
        debug!(
            "Prune deepest node starting key: {:?}, level {}",
            cursor_guard.first_key(),
            level
        );
        // iterate over all removal empty nodes
        // in the beginning cursor_guard contains the first key
        // if the empty node go out of the right bound of current cursor guard,
        // it will `apply_removal` and switch to the next right page
        for (_i, &key_to_del) in removal.empty_pages.iter().enumerate() {
            if key_to_del >= cursor_guard.right_bound() {
                debug!(
                    "Applying removal for overflow current page ({}/{}) key: {:?} >= bound: {:?}. guard keys: {:?}, level {}",
                    guard_removing_poses.len(),
                    cursor_guard.len() + 1,
                    key_to_del,
                    cursor_guard.right_bound(),
                    &cursor_guard.keys()[..cursor_guard.len()],
                    level
                );
                apply_removal(
                    &mut cursor_guard,
                    &mut guard_removing_poses,
                    &mut upper_removal,
                    &prev_key,
                    !deepest,
                );
                cursor_guard = write_targeted(cursor_guard, key_to_del);
                debug_assert!(!cursor_guard.is_empty_node());
                debug!(
                    "Applied removal for overflow current page ({}), level {}",
                    cursor_guard.len(),
                    level
                );
            }
            let pos = cursor_guard.search(key_to_del);
            debug!(
                "Key to delete have position {}, key: {:?}, level {}",
                pos, key_to_del, level
            );
            guard_removing_poses.insert(pos);
            prev_key = Some(key_to_del)
        }
        if !guard_removing_poses.is_empty() {
            debug!(
                "Applying removal for last keys {:?}, level {}",
                &guard_removing_poses, level
            );
            apply_removal(
                &mut cursor_guard,
                &mut guard_removing_poses,
                &mut upper_removal,
                &prev_key,
                !deepest,
            );
        }
    }
    if !removal.split.is_empty() {
        let mut cursor_guard = write_node::<KS, PS>(node);
        for (node, pivot) in &removal.split {
            cursor_guard = write_targeted(cursor_guard, &pivot);
            let pos = cursor_guard.search(&pivot);
            let innode = cursor_guard.innode_mut();
            let pivot = pivot.clone();
            let node = node.clone();
            if innode.len >= KS::slice_len() {
                upper_removal
                    .split
                    .push(innode.split_insert(pivot, node, pos, true));
            } else {
                innode.insert_in_place(pivot, node, pos, true)
            }
        }
    }
    debug!(
        "Have empty nodes {:?}, level {:?}",
        &upper_removal.empty_pages, level
    );
    box upper_removal
}

pub fn level_merge<KS, PS>(src_tree: &BPlusTree<KS, PS>, dest_tree: &LevelTree) -> usize
where
    KS: Slice<EntryKey> + Debug + 'static,
    PS: Slice<NodeCellRef> + 'static,
{
    let mut left_most_leaf_guards = select::<KS, PS>(&src_tree.get_root());
    let merge_page_len = left_most_leaf_guards.len();
    let mut num_keys_removed = 0;
    debug!("Merge selected {} pages", left_most_leaf_guards.len());

    // merge to dest_tree
    {
        let mut deleted_keys = src_tree.deleted.write();
        let mut merged_deleted_keys = vec![];
        let keys: Vec<EntryKey> = left_most_leaf_guards
            .iter()
            .map(|g| &g.keys()[..g.len()])
            .flatten()
            .filter(|&k| {
                if deleted_keys.contains(k) {
                    merged_deleted_keys.push(k.clone());
                    false
                } else {
                    true
                }
            })
            .cloned()
            .collect_vec();
        num_keys_removed = keys.len();
        debug!("Merge selected keys are {:?}", &keys);
        dest_tree.merge_with_keys(box keys);
        for rk in &merged_deleted_keys {
            deleted_keys.remove(rk);
        }
    }

    // cleanup upper level references
    {
        let page_keys = left_most_leaf_guards
            .iter()
            .filter(|g| !g.is_empty())
            .map(|g| g.first_key())
            .collect_vec();
        let mut removal = NodeRemoval::new();
        removal.empty_pages = page_keys;
        prune_selected::<KS, PS>(&src_tree.get_root(), box removal, 0);
    }

    // adjust leaf left, right references
    {
        let right_right_most = left_most_leaf_guards
            .last()
            .unwrap()
            .right_ref()
            .unwrap()
            .clone();

        let left_left_most = left_most_leaf_guards
            .first()
            .unwrap()
            .left_ref()
            .unwrap()
            .clone();

        debug_assert!(read_unchecked::<KS, PS>(&left_left_most).is_none());

        let left_most_id = left_most_leaf_guards.first().unwrap().ext_id();
        for mut g in &mut left_most_leaf_guards {
            external::make_deleted(&g.ext_id());
            **g = NodeData::Empty(box EmptyNode {
                left: Some(left_left_most.clone()),
                right: right_right_most.clone(),
            });
        }

        let mut new_first_node = write_node::<KS, PS>(&right_right_most);
        let mut new_first_node_ext = new_first_node.extnode_mut();
        new_first_node_ext.id = left_most_id;
        new_first_node_ext.prev = left_left_most;

        debug_assert_eq!(new_first_node_ext.id, src_tree.head_page_id);

        ExtNode::<KS, PS>::make_changed(&right_right_most, src_tree);
    }

    src_tree.len.fetch_sub(num_keys_removed, Relaxed);

    merge_page_len
}
