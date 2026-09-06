//! Durable control-plane boundary. Socket senders and routing never enter this store.
use crate::{
    DesiredNode,
    api::{errors::ApiError, models::*},
    registration::{NodeRuntimeStatus, RegistrationServerMessage},
};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::IpAddr, sync::Arc};
use uuid::Uuid;
use validator::Validate;
pub mod postgres;
pub use postgres::PostgresStore;

pub type Store = Arc<dyn ControlStore>;
pub type Result<T> = std::result::Result<T, ApiError>;

#[derive(Clone, Default)]
pub struct Snapshot {
    pub version: i64,
    pub hosts: HashMap<Uuid, Host>,
    pub nodes: HashMap<Uuid, DesiredNode>,
    pub sessions: HashMap<Uuid, Uuid>,
    pub commands: HashMap<Uuid, PendingCommand>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingCommand {
    pub id: Uuid,
    pub host_id: Uuid,
    pub node_id: Uuid,
    pub revision: u64,
    pub message: RegistrationServerMessage,
    pub completed: bool,
}

#[derive(Clone)]
pub enum Mutation {
    Register(Host, Uuid),
    Health(
        Uuid,
        Uuid,
        IpAddr,
        Option<std::collections::BTreeMap<String, serde_json::Value>>,
    ),
    Disconnect(Uuid, Uuid),
    Expire,
    Create(Uuid, Uuid, NodeUpdate),
    Update(Uuid, NodeUpdate),
    Restart(Uuid),
    Status(Uuid, Uuid, Uuid, u64, NodeRuntimeStatus),
}

#[async_trait]
pub trait ControlStore: Send + Sync {
    async fn snapshot(&self) -> Result<Snapshot>;
    /// Validate and commit the state change and its outbox record together.
    async fn update(&self, original: &Snapshot, updated: &Snapshot) -> Result<bool>;
    async fn delete_completed_commands(&self, host_id: Uuid) -> Result<()>;
    /// Get, modify in domain code, then submit the complete changed models.
    /// A failed version check retries against fresh state outside Update.
    async fn mutate(&self, mutation: Mutation) -> Result<Snapshot> {
        for _ in 0..32 {
            let original = self.snapshot().await?;
            let mut updated = original.clone();
            updated.apply(mutation.clone())?;
            updated.version += 1;
            if self.update(&original, &updated).await? {
                return Ok(updated);
            }
            tokio::task::yield_now().await;
        }
        Err(ApiError::conflict(
            "control state changed concurrently; retry the request",
        ))
    }
    async fn hosts(&self) -> Result<Vec<Host>> {
        Ok(self.snapshot().await?.hosts.into_values().collect())
    }
    async fn nodes(&self) -> Result<Vec<Node>> {
        Ok(self
            .snapshot()
            .await?
            .nodes
            .into_values()
            .map(|n| n.node)
            .collect())
    }
    async fn node(&self, id: Uuid) -> Result<Node> {
        self.snapshot()
            .await?
            .nodes
            .remove(&id)
            .map(|n| n.node)
            .ok_or_else(|| ApiError::not_found("node does not exist"))
    }
    async fn create_host(&self, host: Host, session: Uuid) -> Result<Host> {
        let id = host.id;
        Ok(self
            .mutate(Mutation::Register(host, session))
            .await?
            .hosts
            .remove(&id)
            .expect("created host"))
    }
    async fn create_node(&self, host_id: Uuid, node_id: Uuid, input: NodeUpdate) -> Result<Node> {
        Ok(self
            .mutate(Mutation::Create(host_id, node_id, input))
            .await?
            .nodes
            .remove(&node_id)
            .expect("created node")
            .node)
    }
    async fn apply_node_change(&self, node_id: Uuid, input: NodeUpdate) -> Result<Node> {
        Ok(self
            .mutate(Mutation::Update(node_id, input))
            .await?
            .nodes
            .remove(&node_id)
            .expect("updated node")
            .node)
    }
    async fn pending(&self, host_id: Uuid, session: Uuid) -> Result<Vec<PendingCommand>> {
        let state = self.snapshot().await?;
        state.check_session(host_id, session)?;
        let mut first = HashMap::<Uuid, PendingCommand>::new();
        for command in state
            .commands
            .values()
            .filter(|c| c.host_id == host_id && !c.completed)
        {
            if first
                .get(&command.node_id)
                .is_none_or(|old| old.revision > command.revision)
            {
                first.insert(command.node_id, command.clone());
            }
        }
        let mut commands: Vec<_> = first.into_values().collect();
        commands.sort_by_key(|c| (c.node_id, c.revision));
        Ok(commands)
    }
}

impl Snapshot {
    fn check_session(&self, host: Uuid, session: Uuid) -> Result<()> {
        if self.sessions.get(&host) != Some(&session) {
            return Err(ApiError::conflict("stale host session"));
        }
        Ok(())
    }
    fn enqueue(&mut self, node_id: Uuid, restart: bool) {
        let node = &self.nodes[&node_id];
        let message = if restart {
            RegistrationServerMessage::RestartNode {
                node_id,
                revision: node.revision,
            }
        } else {
            RegistrationServerMessage::ApplyNode {
                node_id,
                revision: node.revision,
                update: node.update.clone(),
            }
        };
        let command = PendingCommand {
            id: Uuid::new_v4(),
            host_id: node.host_id,
            node_id,
            revision: node.revision,
            message,
            completed: false,
        };
        self.commands.insert(command.id, command);
    }
    fn offline(&mut self, host: Uuid) {
        self.sessions.remove(&host);
        if let Some(host) = self.hosts.get_mut(&host) {
            host.status = HostStatus::Offline;
        }
        for node in self.nodes.values_mut().filter(|n| n.host_id == host) {
            node.node.set_status(NodeStatus::Disconnected);
        }
    }
    pub(crate) fn apply(&mut self, mutation: Mutation) -> Result<()> {
        match mutation {
            Mutation::Register(host, session) => {
                let host_id = host.id;
                self.hosts.insert(host_id, host);
                self.sessions.insert(host_id, session);
                // A fresh runtime needs the latest complete configuration, even when
                // the last queued action was a restart. Supersede historical work.
                for command in self.commands.values_mut().filter(|c| c.host_id == host_id) {
                    command.completed = true;
                }
                let ids: Vec<_> = self
                    .nodes
                    .iter_mut()
                    .filter(|(_, n)| n.host_id == host_id)
                    .map(|(id, n)| {
                        n.revision += 1;
                        n.node.set_status(NodeStatus::Disconnected);
                        *id
                    })
                    .collect();
                for id in ids {
                    self.enqueue(id, false);
                }
            }
            Mutation::Health(host_id, session, ip, metadata) => {
                self.check_session(host_id, session)?;
                let host = self.hosts.get_mut(&host_id).expect("session has host");
                host.ip = ip;
                host.metadata = metadata;
                host.last_seen_at = Utc::now();
                host.status = HostStatus::Online;
            }
            Mutation::Disconnect(host, session) => {
                self.check_session(host, session)?;
                self.offline(host);
            }
            Mutation::Expire => {
                let expired: Vec<_> = self
                    .hosts
                    .values()
                    .filter(|h| {
                        h.status != HostStatus::Offline
                            && h.last_seen_at < Utc::now() - Duration::seconds(45)
                    })
                    .map(|h| h.id)
                    .collect();
                for host in expired {
                    self.offline(host);
                }
            }
            Mutation::Create(host_id, id, update) => {
                update.validate().map_err(ApiError::validation)?;
                let host = self
                    .hosts
                    .get(&host_id)
                    .cloned()
                    .ok_or_else(|| ApiError::not_found("host does not exist"))?;
                crate::api::nodes::validate_input(id, &update, &self.nodes)?;
                if self.nodes.contains_key(&id) {
                    return Err(ApiError::conflict("node already exists"));
                }
                let node = Node::from_update(id, host, &update);
                self.nodes.insert(
                    id,
                    DesiredNode {
                        host_id,
                        revision: 1,
                        update,
                        node,
                    },
                );
                self.enqueue(id, false);
            }
            Mutation::Update(id, update) => {
                update.validate().map_err(ApiError::validation)?;
                crate::api::nodes::validate_input(id, &update, &self.nodes)?;
                let desired = self
                    .nodes
                    .get_mut(&id)
                    .ok_or_else(|| ApiError::not_found("node does not exist"))?;
                if desired.node.node_type() != update.node_type() {
                    return Err(ApiError::conflict("node type is immutable"));
                }
                desired.revision += 1;
                desired.node = desired.node.with_update(&update);
                desired.update = update;
                self.enqueue(id, false);
            }
            Mutation::Restart(id) => {
                let desired = self
                    .nodes
                    .get_mut(&id)
                    .ok_or_else(|| ApiError::not_found("node does not exist"))?;
                if !desired.node.enabled() {
                    return Err(ApiError::conflict("disabled nodes cannot be restarted"));
                }
                desired.revision += 1;
                desired.node.set_status(NodeStatus::Disconnected);
                self.enqueue(id, true);
            }
            Mutation::Status(host, session, id, revision, status) => {
                self.check_session(host, session)?;
                if let Some(node) = self.nodes.get_mut(&id).filter(|n| n.host_id == host) {
                    if node.revision == revision {
                        node.node.set_status(match status {
                            NodeRuntimeStatus::Running => NodeStatus::Connected,
                            NodeRuntimeStatus::Failed => NodeStatus::Degraded,
                            _ => NodeStatus::Disconnected,
                        });
                    }
                    // Older revisions still acknowledge their own outbox entry, but
                    // must never overwrite the latest desired document's status.
                    for command in self
                        .commands
                        .values_mut()
                        .filter(|c| c.node_id == id && c.revision == revision)
                    {
                        let enabled = match &command.message {
                            RegistrationServerMessage::ApplyNode { update, .. } => update.enabled(),
                            _ => true,
                        };
                        command.completed |= status == NodeRuntimeStatus::Failed
                            || (enabled && status == NodeRuntimeStatus::Running)
                            || (!enabled && status == NodeRuntimeStatus::Stopped);
                    }
                }
            }
        }
        for desired in self.nodes.values_mut() {
            if let Some(host) = self.hosts.get(&desired.host_id) {
                desired.node.base_mut().host = host.clone();
            }
        }
        Ok(())
    }
}

/// Deterministic test adapter; production binaries always construct PostgresStore.
#[derive(Default)]
pub struct MemoryStore(tokio::sync::RwLock<Snapshot>);
#[async_trait]
impl ControlStore for MemoryStore {
    async fn snapshot(&self) -> Result<Snapshot> {
        Ok(self.0.read().await.clone())
    }
    async fn update(&self, original: &Snapshot, updated: &Snapshot) -> Result<bool> {
        let mut guard = self.0.write().await;
        if guard.version != original.version {
            return Ok(false);
        }
        *guard = updated.clone();
        guard.version = original.version + 1;
        Ok(true)
    }
    async fn delete_completed_commands(&self, host_id: Uuid) -> Result<()> {
        self.0
            .write()
            .await
            .commands
            .retain(|_, c| c.host_id != host_id || !c.completed);
        Ok(())
    }
}
