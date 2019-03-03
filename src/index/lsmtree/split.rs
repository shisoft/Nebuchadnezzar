use dovahkiin::types::custom_types::id::Id;
use index::lsmtree::tree::LSMTree;
use index::Cursor;
use index::EntryKey;
use index::Ordering::Forward;
use itertools::Itertools;
use rayon::prelude::*;

pub struct SplitStatus {
    start: EntryKey,
    target: Id,
}

pub fn mid_key(tree: &LSMTree) -> EntryKey {
    // TODO: more accurate mid key take account of all tree levels
    // Current implementation only take the mid key from the tree with the most number of keys
    tree.trees
        .iter()
        .map(|tree| (tree.mid_key(), tree.count()))
        .filter_map(|(mid, count)| mid.map(|mid| (mid, count)))
        .max_by_key(|(mid, count)| *count)
        .map(|(mid, _)| mid)
        .unwrap()
}

pub fn check_and_split(tree: &LSMTree) -> bool {
    if tree.is_full() && tree.split.lock().is_none() {
        // need to initiate a split
        let tree_key_range = tree.range.lock().clone();
        let mid_key = mid_key(tree);
        let new_tree_range = (mid_key, tree_key_range.0.clone());
        // First take a new tree metadata generated by the placement driver
        unimplemented!();
        // Then save this metadata to current tree 'split' field
        unimplemented!();
        // Inform the placement driver that this tree is going to split so it can direct all write
        // and read request to the new tree
        unimplemented!();
    }
    let mut tree_split = tree.split.lock();
    // check if current tree is in the middle of split, so it can (re)start from the process
    if let Some(tree_split) = &*tree_split {
        // Get a cursor from mid key, forwarding keys
        let mut cursor = tree.seek(tree_split.start.clone(), Forward);
        let batch_size = tree.last_level_size();
        while cursor.current().is_some() {
            let mut batch = Vec::with_capacity(batch_size);
            while batch.len() < batch_size && cursor.current().is_some() {
                batch.push(cursor.current().unwrap().clone());
                cursor.next();
            }
            // submit this batch to new tree
            unimplemented!();
            // remove this batch in current tree
            unimplemented!();
        }
        // split completed
        tree.remove_following_tombstones(&tree_split.start);
        // Set new tree epoch from 0 to 1
        unimplemented!();
        // Inform the placement driver this tree have completed split
        unimplemented!();
    } else {
        return false;
    }
    *tree_split = None;
    true
}
