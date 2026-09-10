//! Host presence and host-owned resource cleanup.
use super::{
    Context, ReconcileError, error_policy, finalizer_error,
    session::{EXPIRY, State},
};
use crate::kubernetes::{Error, resources::*};
use futures_util::StreamExt;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use kube::{
    Api, ResourceExt,
    api::{DeleteParams, ListParams},
    runtime::{
        controller::{Action, Config, Controller as KubeController},
        finalizer::{self, Event},
        watcher,
    },
};
use std::sync::Arc;

/// Run host presence and owned-node cleanup reconciliation.
pub(super) async fn run(hosts: Api<SuntitleHost>, nodes: Api<SuntitleNode>, context: Arc<Context>) {
    KubeController::new(hosts, watcher::Config::default())
        .owns(nodes, watcher::Config::default())
        .with_config(Config::default().concurrency(1))
        .run(reconcile, error_policy, context)
        .for_each(|result| async move {
            if let Err(error) = result {
                eprintln!("host controller: {error}");
            }
        })
        .await;
}

async fn reconcile(host: Arc<SuntitleHost>, ctx: Arc<Context>) -> Result<Action, ReconcileError> {
    let mut state = ctx.state.lock().await;
    let api = state.hosts_api.clone();
    finalizer::finalizer(&api, HOST_FINALIZER, host, |event| async {
        match event {
            Event::Apply(host) => {
                state
                    .reconcile_host(&host)
                    .await
                    .map_err(ReconcileError::Failed)?;
                Ok(Action::requeue(EXPIRY))
            }
            Event::Cleanup(host) => {
                if state
                    .cleanup_host(&host)
                    .await
                    .map_err(ReconcileError::Failed)?
                {
                    Ok(Action::await_change())
                } else {
                    Err(ReconcileError::CleanupPending)
                }
            }
        }
    })
    .await
    .map_err(finalizer_error)
}

impl State {

    /// The standalone API server has no garbage collector for owned nodes.
    async fn cleanup_host(&mut self, host: &SuntitleHost) -> Result<bool, Error> {
        let nodes = self.nodes_api.list(&ListParams::default()).await?;
        let owned: Vec<_> = nodes
            .iter()
            .filter(|node| {
                node.owner_references().iter().any(|owner| {
                    owner.controller == Some(true)
                        && Some(owner.uid.as_str()) == host.metadata.uid.as_deref()
                })
            })
            .collect();
        if owned.is_empty() {
            self.close(host.metadata.uid.as_deref().unwrap());
            return Ok(true);
        }
        for node in owned {
            if node.metadata.deletion_timestamp.is_none() {
                self.nodes_api
                    .delete(&node.name_any(), &DeleteParams::default())
                    .await?;
            }
        }
        Ok(false)
    }

    async fn reconcile_host(&mut self, host: &SuntitleHost) -> Result<(), Error> {
        let uid = host.metadata.uid.clone().unwrap();
        let mut host = host.clone();
        let generation = host.metadata.generation.unwrap_or(1);
        let previous = host.status.clone();
        let status = host.status.get_or_insert_default();
        let updated = status.conditions.iter().any(|c| {
            c.type_ == "Updated" && c.status == "True" && c.observed_generation == Some(generation)
        });
        if !updated {
            status.conditions.push(Condition {
                last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
                observed_generation: Some(generation),
                type_: "Updated".into(),
                status: "True".into(),
                reason: "DesiredStateApplied".into(),
                message: "Host configuration accepted.".into(),
            });
        }
        let presence = if self.active_session(&host).is_some() {
            Presence::Online
        } else {
            Presence::Offline
        };
        let status = host.status.get_or_insert_default();
        status.conditions.push(presence.ready_condition(generation));
        let offline = matches!(presence, Presence::Offline);
        status.presence = Some(presence);
        self.host_status(&mut host, &previous).await?;
        if offline
            && host
                .status
                .as_ref()
                .and_then(|status| status.session_id.as_deref())
                .is_some_and(|session| self.connections.contains(&uid, session))
        {
            self.close(&uid);
        }
        Ok(())
    }
}
