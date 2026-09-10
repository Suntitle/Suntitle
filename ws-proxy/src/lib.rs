use uuid::Uuid;

pub mod api;
pub mod controller;
pub mod diamant;
pub mod kubernetes;
pub mod registration;
mod transport;

use api::models::{Node, NodeUpdate};
pub mod storage;

#[derive(Debug, Clone, serde::Serialize)]
pub struct DesiredNode {
    pub host_id: Uuid,
    pub revision: u64,
    pub update: NodeUpdate,
    pub node: Node,
}
