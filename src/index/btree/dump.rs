use index::btree::BPlusTree;
use index::Slice;
use index::EntryKey;
use index::btree::NodeCellRef;
use std::fs::File;
use std::fmt::Debug;
use index::btree::node::read_unchecked;
use index::btree::node::NodeData;
use index::btree::external::ExtNode;
use index::id_from_key;
use std::io::Write;
use serde_json;

#[derive(Serialize, Deserialize)]
struct DebugNode {
    keys: Vec<String>,
    nodes: Vec<DebugNode>,
    id: Option<String>,
    next: Option<String>,
    prev: Option<String>,
    len: usize,
    is_external: bool,
}

pub fn dump_tree<KS, PS>(tree: &BPlusTree<KS, PS>, f: &str)
    where
        KS: Slice<EntryKey> + Debug + 'static,
        PS: Slice<NodeCellRef> + 'static,
{
    debug!("dumping {}", f);
    let debug_root = cascading_dump_node::<KS, PS>(&tree.get_root());
    let json = serde_json::to_string_pretty(&debug_root).unwrap();
    let mut file = File::create(f).unwrap();
    file.write_all(json.as_bytes());
}

fn cascading_dump_node<KS, PS>(node: &NodeCellRef) -> DebugNode
    where
        KS: Slice<EntryKey> + Debug + 'static,
        PS: Slice<NodeCellRef> + 'static,
{
    if node.is_default() {
        return DebugNode {
            keys: vec![String::from("<ERROR!!! DEFAULT NODE!!!>")],
            nodes: vec![],
            id: None,
            next: None,
            prev: None,
            len: 0,
            is_external: false,
        };
    }
    let node = read_unchecked(&*node);
    match &*node {
        &NodeData::External(ref node) => {
            let node: &ExtNode<KS, PS> = node;
            let keys = node.keys.as_slice_immute()[..node.len]
                .iter()
                .map(|key| {
                    let id = id_from_key(key);
                    format!("{}\t{:?}", id.lower, key)
                })
                .collect();
            return DebugNode {
                keys,
                nodes: vec![],
                id: Some(format!("{:?}", node.id)),
                next: Some(format!(
                    "{:?}",
                    read_unchecked::<KS, PS>(&node.next).ext_id()
                )),
                prev: Some(format!(
                    "{:?}",
                    read_unchecked::<KS, PS>(&node.prev).ext_id()
                )),
                len: node.len,
                is_external: true,
            };
        }
        &NodeData::Internal(ref innode) => {
            let len = innode.len;
            let keys = innode.keys.as_slice_immute()[..node.len()]
                .iter()
                .map(|key| format!("{:?}", key))
                .collect();
            let nodes = innode.ptrs.as_slice_immute()[..node.len() + 1]
                .iter()
                .map(|node_ref| cascading_dump_node::<KS, PS>(node_ref))
                .collect();
            return DebugNode {
                keys,
                nodes,
                id: None,
                next: Some(innode.right.to_string::<KS, PS>()),
                prev: None,
                len,
                is_external: false,
            };
        }
        &NodeData::None => {
            return DebugNode {
                keys: vec![String::from("<NOT FOUND>")],
                nodes: vec![],
                id: None,
                next: None,
                prev: None,
                len: 0,
                is_external: false,
            };
        }
        &NodeData::Empty(ref n) => {
            return DebugNode {
                keys: vec![String::from("<EMPTY>")],
                nodes: vec![],
                id: None,
                next: Some(n.right.to_string::<KS, PS>()),
                prev: None,
                len: 0,
                is_external: false,
            };
        }
    }
}