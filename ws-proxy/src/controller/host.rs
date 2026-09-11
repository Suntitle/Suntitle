//! Host presence reconciliation.
use super::{
    Context, ReconcileError, error_policy,
    session::{EXPIRY, State},
};
use crate::kubernetes::{Error, resources::*};
use futures_util::StreamExt;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use kube::{
    Api,
    runtime::{
        controller::{Action, Config, Controller as KubeController},
        watcher,
    },
};
use std::sync::Arc;

/// Reconcile host presence.
pub(super) async fn run(hosts: Api<SuntitleHost>, context: Arc<Context>) {
    KubeController::new(hosts, watcher::Config::default())
        .with_config(Config::default().concurrency(1))
        .run(reconcile, error_policy, context)
        .for_each(|result| async move {
            if let Err(error) = result {
                eprintln!("host controller: {error}");
            }
        })
        .await;
}

/// Refresh host presence and schedule an expiry check.
async fn reconcile(host: Arc<SuntitleHost>, ctx: Arc<Context>) -> Result<Action, ReconcileError> {
    ctx.state
        .lock()
        .await
        .reconcile_host(&host)
        .await
        .map_err(ReconcileError::Failed)?;
    Ok(Action::requeue(EXPIRY))
}

impl State {
    /// Update host conditions and close expired sessions.
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
