//! Runtime dispatch, acknowledgement evaluation, and node finalization.
use super::{
    Context, ReconcileError, error_policy, finalizer_error,
    lifecycle::set_phase,
    plan::Plan,
    session::{State, identity},
};
use futures_util::StreamExt;
use kube::{
    Api, ResourceExt,
    runtime::{
        controller::{Action, Config, Controller as KubeController},
        finalizer::{self, Event},
        reflector::ObjectRef,
        watcher,
    },
};
use std::sync::Arc;
use suntitle_core::{
    kubernetes::{Error, resources::*},
    registration::*,
};

/// Observe node status and host sessions independently of spec revisions.
pub(super) async fn run(nodes: Api<SuntitleNode>, hosts: Api<SuntitleHost>, context: Arc<Context>) {
    let controller = KubeController::new(nodes, watcher::Config::default());
    let store = controller.store();
    controller
        .with_config(Config::default().concurrency(1))
        .watches(hosts, watcher::Config::default(), move |host| {
            store
                .state()
                .iter()
                .filter(|node| node.spec.host_ref.name == host.name_any())
                .map(|node| ObjectRef::from_obj(node.as_ref()))
                .collect::<Vec<_>>()
        })
        .run(reconcile, error_policy, context)
        .for_each(|result| async move {
            if let Err(error) = result {
                eprintln!("runtime controller: {error}");
            }
        })
        .await;
}

/// Reconcile node runtime state and removal finalization.
async fn reconcile(node: Arc<SuntitleNode>, ctx: Arc<Context>) -> Result<Action, ReconcileError> {
    let mut state = ctx.state.lock().await;
    let api = state.nodes_api.clone();
    finalizer::finalizer(&api, NODE_FINALIZER, node, |event| async {
        match event {
            Event::Apply(node) => {
                state
                    .reconcile_runtime(&node)
                    .await
                    .map_err(ReconcileError::Failed)?;
                Ok(Action::await_change())
            }
            Event::Cleanup(node) => {
                if cleanup_complete(&node) {
                    return Ok(Action::await_change());
                }
                state
                    .reconcile_runtime(&node)
                    .await
                    .map_err(ReconcileError::Failed)?;
                Err(ReconcileError::CleanupPending)
            }
        }
    })
    .await
    .map_err(finalizer_error)
}
impl State {
    /// Track dispatches on the current connection to resynchronize after reconnect.
    pub(super) async fn reconcile_runtime(&mut self, node: &SuntitleNode) -> Result<(), Error> {
        let Some(plan) = Plan::for_node(node) else {
            return Ok(());
        };
        let Some(dispatch) = node
            .status
            .as_ref()
            .and_then(|status| status.dispatch.as_ref())
        else {
            return Ok(());
        };
        if !plan.matches(node, dispatch) {
            return Ok(());
        }
        let host = self.hosts_api.get(&node.spec.host_ref.name).await?;
        let host_uid = host.metadata.uid.as_deref().unwrap();
        if !host.finalizers().iter().any(|f| f == HOST_FINALIZER)
            || (host.metadata.deletion_timestamp.is_some() && !plan.is_removal())
        {
            return Ok(());
        }
        let mut node = node.clone();
        let previous = node.status.clone();
        let Some(session) = self.active_session(&host) else {
            set_phase(&mut node, plan.offline_phase());
            return self.node_status(&mut node, &previous).await;
        };
        let status = node.status.as_mut().unwrap();
        if matches!(plan, Plan::Apply { .. })
            && status.reporting_session.as_deref() != Some(&session)
        {
            for condition in &mut status.conditions {
                if condition.type_ == "Installed" {
                    condition.status = "False".into();
                    condition.reason = "RestoringInstallation".into();
                    condition.message =
                        "Restore the prepared payload for the current host session.".into();
                }
            }
            status.dispatch = None;
            set_phase(&mut node, NodePhase::Pending);
            return self.node_status(&mut node, &previous).await;
        }
        let dispatch = status.dispatch.as_mut().unwrap();
        if dispatch.session_id.as_deref() == Some(&session)
            && self.connections.dispatched(
                host_uid,
                node.metadata.uid.as_deref().unwrap(),
                &dispatch.operation_id,
            )
        {
            if let Some(success) = dispatch.success {
                let operation = dispatch.operation;
                super::lifecycle::acknowledge(&mut node, operation, success);
                self.node_status(&mut node, &previous).await?;
            }
            return Ok(());
        }
        dispatch.session_id = Some(session.clone());
        dispatch.operation_id = uuid::Uuid::new_v4().to_string();
        dispatch.success = None;
        let identity = ApplyIdentity {
            target: identity(&node.metadata),
            host_uid: host_uid.into(),
            session_id: session.clone(),
            generation: dispatch.generation,
            restart_token: node.spec.restart_token.clone(),
            operation_id: dispatch.operation_id.clone(),
        };
        status.message = None;
        set_phase(&mut node, plan.pending_phase());
        let operation_id = identity.operation_id.clone();
        let node_uid = identity.target.uid.clone();
        self.node_status(&mut node, &previous).await?;
        let message = plan.into_message(identity, node.spec);
        self.dispatch(host_uid, &session, message);
        self.connections
            .mark_dispatched(host_uid, node_uid, operation_id);
        Ok(())
    }
}

/// An apply acknowledgement cannot complete cleanup.
fn cleanup_complete(node: &SuntitleNode) -> bool {
    node.status
        .as_ref()
        .and_then(|status| status.dispatch.as_ref())
        .is_some_and(|dispatch| {
            dispatch.operation == Operation::Remove
                && dispatch.generation == node.metadata.generation.unwrap()
                && dispatch.success == Some(true)
        })
}
