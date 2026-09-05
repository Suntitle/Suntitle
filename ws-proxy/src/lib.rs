use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use uuid::Uuid;

pub mod api;
pub mod registration;

use api::models::Host;

/// Hosts known to the currently running proxy process, keyed by stable host UUID.
pub type HostRegistry = Arc<RwLock<HashMap<Uuid, Host>>>;
