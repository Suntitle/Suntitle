use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, mpsc};
use uuid::Uuid;

pub mod api;
pub mod diamant;
pub mod registration;

use api::models::{Host, Node, NodeUpdate};
use registration::RegistrationServerMessage;

/// Hosts known to the currently running proxy process, keyed by stable host UUID.
pub type HostRegistry = Arc<RwLock<HashMap<Uuid, Host>>>;

#[derive(Debug, Clone)]
pub struct DesiredNode {
    pub host_id: Uuid,
    pub revision: u64,
    pub update: NodeUpdate,
    pub node: Node,
}

pub type NodeRegistry = Arc<RwLock<HashMap<Uuid, DesiredNode>>>;
pub type HostControlRegistry =
    Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<RegistrationServerMessage>>>>;

#[derive(Clone, Default)]
pub struct ControlPlaneState {
    pub hosts: HostRegistry,
    pub nodes: NodeRegistry,
    pub host_controls: HostControlRegistry,
}
