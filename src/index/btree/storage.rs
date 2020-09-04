use crate::client;
use crate::index::btree::{external};
use std::time::Duration;
use std::sync::Arc;

pub fn start_external_nodes_write_back(client: &Arc<client::AsyncClient>) {
    let client = client.clone();
    tokio::spawn(async move {
        loop {
            while let Ok(changing) = external::CHANGED_NODES.pop() {
                match changing {
                    external::ChangingNode::Modified(modified) => {
                        modified.node.persist(&modified.deletion, &client).await;
                    },
                    external::ChangingNode::Deleted(id) => {
                        client.remove_cell(id).await.unwrap().unwrap();
                    }
                }
            }
            tokio::time::delay_for(Duration::from_millis(500)).await;
        }
    });
}
