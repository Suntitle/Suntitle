//! Runtime dispatch, acknowledgement evaluation, and node finalization.
use super::{
    Context, ReconcileError, dispatch, error_policy, finalizer_error,
    session::{State, identity},
};
use crate::{
    kubernetes::{Error, resources::*},
    registration::*,
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

/// Observe node status and host sessions independently of spec revisions.
pub(super) async fn run(
    nodes: Api<SuntitleNode>,
    hosts: Api<SuntitleHost>,
    context: Arc<Context>,
) {
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
    async fn reconcile_runtime(&mut self, node: &SuntitleNode) -> Result<(), Error> {
        if node.spec.extension_ref.is_some() && node.metadata.deletion_timestamp.is_none() {
            return Ok(());
        }
        let Some(dispatch) = node
            .status
            .as_ref()
            .and_then(|status| status.dispatch.as_ref())
        else {
            return Ok(());
        };
        if dispatch.generation != node.metadata.generation.unwrap()
            || dispatch.operation != dispatch::operation(node)
        {
            return Ok(());
        }
        let removing = dispatch.operation == Operation::Remove;
        let host = self.hosts_api.get(&node.spec.host_ref.name).await?;
        let host_uid = host.metadata.uid.as_deref().unwrap();
        if !host.finalizers().iter().any(|f| f == HOST_FINALIZER)
            || (host.metadata.deletion_timestamp.is_some() && !removing)
        {
            return Ok(());
        }
        let mut node = node.clone();
        let previous = node.status.clone();
        let Some(session) = self.active_session(&host) else {
            set_phase(
                &mut node,
                if removing {
                    NodePhase::Terminating
                } else {
                    NodePhase::Disconnected
                },
            );
            return self.node_status(&mut node, &previous).await;
        };
        let status = node.status.as_mut().unwrap();
        let dispatch = status.dispatch.as_mut().unwrap();
        if dispatch.session_id.as_deref() == Some(&session)
            && self.connections.dispatched(
                host_uid,
                node.metadata.uid.as_deref().unwrap(),
                &dispatch.operation_id,
            )
        {
            if let Some(success) = dispatch.success {
                if success {
                    status.observed_generation = Some(dispatch.generation);
                    status.reporting_session = Some(session);
                    status.acknowledged_restart_token = node.spec.restart_token.clone();
                }
                let phase = if removing {
                    NodePhase::Terminating
                } else {
                    NodePhase::from_acknowledgement(success, node.spec.enabled)
                };
                set_phase(&mut node, phase);
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
        set_phase(
            &mut node,
            if removing {
                NodePhase::Terminating
            } else {
                NodePhase::Pending
            },
        );
        let operation_id = identity.operation_id.clone();
        let node_uid = identity.target.uid.clone();
        self.node_status(&mut node, &previous).await?;
        let message = if removing {
            RegistrationServerMessage::Remove { identity }
        } else {
            RegistrationServerMessage::Apply {
                identity,
                spec: Box::new(node.spec),
            }
        };
        self.dispatch(host_uid, &session, message);
        self.connections.mark_dispatched(host_uid, node_uid, operation_id);
        Ok(())
    }
}

/// Update the node phase and readiness conditions.
fn set_phase(node: &mut SuntitleNode, phase: NodePhase) {
    let generation = node.metadata.generation.unwrap();
    let status = node.status.as_mut().unwrap();
    status
        .conditions
        .push(phase.ready_condition(generation, status.message.as_deref()));
    status
        .conditions
        .push(phase.updated_condition(generation, status.message.as_deref()));
    status.phase = Some(phase);
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
