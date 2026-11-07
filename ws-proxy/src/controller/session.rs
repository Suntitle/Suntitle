//! Runtime registration, reports, and shared resource status writes.
use super::{Connection, conditions};
use crate::transport::Connections;
use chrono::Utc;
use kube::{
    Api, ResourceExt,
    api::{Patch, PatchParams, PostParams},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
use suntitle_core::{
    kubernetes::{Error, resources::*},
    registration::*,
};

pub(super) const EXPIRY: Duration = Duration::from_secs(45);

/// Kubernetes clients and active host connections.
pub(super) struct State {
    /// Client for host resources and status.
    pub(super) hosts_api: Api<SuntitleHost>,
    /// Client for node resources and status.
    pub(super) nodes_api: Api<SuntitleNode>,
    /// Active host sessions and dispatch tracking.
    pub(super) connections: Connections,
}

impl State {
    /// Close the matching session and persist offline status.
    pub(super) async fn disconnect(&mut self, uid: &str, session: &str) -> Result<(), Error> {
        let Some(identity) = self.connections.host(uid) else {
            return Ok(());
        };
        if !self.connections.contains(uid, session) {
            return Ok(());
        }
        let host = self.hosts_api.get_opt(&identity.name).await;
        self.close(uid);
        if let Some(host) = host?.filter(|host| current_session(host, uid, session)) {
            self.offline(&host).await?;
        }
        Ok(())
    }

    /// Reports update status; reconciliation owns finalizer release.
    pub(super) async fn report(
        &mut self,
        host_uid: &str,
        session_id: &str,
        message: RegistrationClientMessage,
    ) -> Result<(), Error> {
        let Some(connection) = self.connections.host(host_uid) else {
            return Ok(());
        };
        let Some(mut host) = self.hosts_api.get_opt(&connection.name).await? else {
            self.close(host_uid);
            return Ok(());
        };
        if !current_session(&host, host_uid, session_id)
            || !self.connections.contains(host_uid, session_id)
        {
            return Ok(());
        }
        match message {
            RegistrationClientMessage::Heartbeat {
                host_uid: uid,
                session_id: id,
            } if uid == host_uid && id == session_id => {
                let previous = host.status.clone();
                let status = host.status.get_or_insert_default();
                status.last_heartbeat = Some(Utc::now());
                status.presence = Some(Presence::Online);
                self.host_status(&mut host, &previous).await?;
            }
            RegistrationClientMessage::Acknowledgement {
                identity,
                operation,
                success,
                message,
            } => {
                if identity.host_uid != host_uid || identity.session_id != session_id {
                    return Ok(());
                }
                let Some(mut node) = self.nodes_api.get_opt(&identity.target.name).await? else {
                    return Ok(());
                };
                let Some(dispatch) = node
                    .status
                    .as_ref()
                    .and_then(|status| status.dispatch.as_ref())
                else {
                    return Ok(());
                };
                if dispatch.operation_id != identity.operation_id
                    || dispatch.operation != operation
                    || dispatch.session_id.as_deref() != Some(session_id)
                    || !matches_identity(&node, &identity)
                    || !super::plan::Plan::for_node(&node)
                        .is_some_and(|plan| plan.matches(&node, dispatch))
                    || node.spec.host_ref.name != host.name_any()
                {
                    return Ok(());
                }
                let previous = node.status.clone();
                let status = node.status.as_mut().unwrap();
                status.dispatch.as_mut().unwrap().success = Some(success);
                status.message = message;
                self.node_status(&mut node, &previous).await?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Create or reuse a host using a lowercase DNS name and replace its connection.
    /// The display name MUST retain the agent's original hostname.
    pub(super) async fn register(
        &mut self,
        name: String,
        version: String,
        metadata: BTreeMap<String, Value>,
        ip: Option<String>,
    ) -> Result<Connection, Error> {
        if name.is_empty() || version.trim().is_empty() {
            return Err("host name and agent version are required".into());
        }
        let resource_name = name.to_ascii_lowercase();
        let mut host = match self.hosts_api.get_opt(&resource_name).await? {
            Some(host) => host,
            None => {
                self.hosts_api
                    .create(
                        &PostParams::default(),
                        &SuntitleHost::new(
                            &resource_name,
                            SuntitleHostSpec {
                                display_name: name.clone(),
                            },
                        ),
                    )
                    .await?
            }
        };
        let identity = identity(&host.metadata);
        let connection = self.connections.open(identity.clone());
        let previous = host.status.clone();
        let status = host.status.get_or_insert_default();
        status.ip = ip;
        status.agent_version = Some(version);
        status.reported_metadata = metadata;
        status.session_id = Some(connection.session_id.clone());
        status.last_heartbeat = Some(Utc::now());
        status.presence = Some(Presence::Online);
        if let Err(error) = self.host_status(&mut host, &previous).await {
            self.close(&identity.uid);
            return Err(error);
        }
        Ok(connection)
    }

    /// Close the host connection.
    pub(super) fn close(&mut self, uid: &str) {
        self.connections.close(uid);
    }

    /// Persist offline presence and close the host connection.
    pub(super) async fn offline(&mut self, host: &SuntitleHost) -> Result<(), Error> {
        let mut host = host.clone();
        let previous = host.status.clone();
        let status = host.status.get_or_insert_default();
        status.presence = Some(Presence::Offline);
        status
            .conditions
            .push(Presence::Offline.ready_condition(host.metadata.generation.unwrap_or(1)));
        self.host_status(&mut host, &previous).await?;
        self.close(host.metadata.uid.as_deref().unwrap());
        Ok(())
    }

    /// Return the connected session with a fresh heartbeat.
    pub(super) fn active_session(&self, host: &SuntitleHost) -> Option<String> {
        if heartbeat_expired(host, Utc::now()) {
            return None;
        }
        let session = host.status.as_ref()?.session_id.as_ref()?;
        self.connections
            .contains(host.metadata.uid.as_deref().unwrap(), session)
            .then(|| session.clone())
    }

    /// Persist changed host status with merged conditions.
    pub(super) async fn host_status(
        &self,
        host: &mut SuntitleHost,
        previous: &Option<HostStatus>,
    ) -> Result<(), Error> {
        if let Some(status) = host.status.as_mut() {
            conditions::merge(
                previous
                    .as_ref()
                    .map(|status| status.conditions.as_slice())
                    .unwrap_or_default(),
                &mut status.conditions,
            );
        }
        if json!(previous) != json!(host.status) {
            *host = self.hosts_api.patch_status(&host.name_any(), &PatchParams::default(), &Patch::Merge(json!({"metadata":{"resourceVersion":host.resource_version(),"uid":host.uid()},"status":host.status}))).await?;
        }
        Ok(())
    }

    /// Persist changed node status with merged conditions.
    pub(super) async fn node_status(
        &self,
        node: &mut SuntitleNode,
        previous: &Option<NodeStatus>,
    ) -> Result<(), Error> {
        if let Some(status) = node.status.as_mut() {
            conditions::merge(
                previous
                    .as_ref()
                    .map(|status| status.conditions.as_slice())
                    .unwrap_or_default(),
                &mut status.conditions,
            );
        }
        if json!(previous) != json!(node.status) {
            // Nulls MUST clear optional fields omitted by status serialization.
            let mut value = json!(node.status);
            for field in ["message", "reportingSession", "acknowledgedRestartToken"] {
                if value.get(field).is_none() {
                    value[field] = Value::Null;
                }
            }
            *node = self.nodes_api.patch_status(&node.name_any(), &PatchParams::default(), &Patch::Merge(json!({"metadata":{"resourceVersion":node.resource_version(),"uid":node.uid()},"status":value}))).await?;
        }
        Ok(())
    }

    /// Queue overflow closes the session so reconnect can resynchronize.
    pub(super) fn dispatch(
        &mut self,
        host_uid: &str,
        session: &str,
        message: RegistrationServerMessage,
    ) {
        if !self.connections.send(host_uid, session, message)
            && self.connections.contains(host_uid, session)
        {
            self.close(host_uid);
        }
    }
}

/// Check the host UID and recorded session.
pub(super) fn current_session(host: &SuntitleHost, uid: &str, session: &str) -> bool {
    host.uid().as_deref() == Some(uid)
        && host
            .status
            .as_ref()
            .and_then(|status| status.session_id.as_deref())
            == Some(session)
}

/// Check whether the heartbeat is missing or too old.
pub(super) fn heartbeat_expired(host: &SuntitleHost, now: chrono::DateTime<Utc>) -> bool {
    host.status
        .as_ref()
        .and_then(|status| status.last_heartbeat)
        .is_none_or(|heartbeat| {
            now.signed_duration_since(heartbeat)
                >= chrono::Duration::from_std(EXPIRY)
                    .expect("heartbeat expiry fits chrono duration")
        })
}

/// Extract the resource namespace, name, and UID.
pub(super) fn identity(metadata: &kube::core::ObjectMeta) -> ResourceIdentity {
    ResourceIdentity {
        namespace: metadata.namespace.clone().unwrap(),
        name: metadata.name.clone().unwrap(),
        uid: metadata.uid.clone().unwrap(),
    }
}

/// Check report identity for a node without pending deletion.
#[cfg(test)]
pub(super) fn matches_node(node: &SuntitleNode, id: &ApplyIdentity) -> bool {
    matches_identity(node, id) && node.metadata.deletion_timestamp.is_none()
}

/// Check the resource identity, generation, and restart token.
pub(super) fn matches_identity(node: &SuntitleNode, id: &ApplyIdentity) -> bool {
    node.uid().as_deref() == Some(&id.target.uid)
        && node.namespace().as_deref() == Some(&id.target.namespace)
        && node.name_any() == id.target.name
        && node.metadata.generation == Some(id.generation)
        && node.spec.restart_token == id.restart_token
}
