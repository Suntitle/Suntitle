//! Generation/deletion-driven dispatch intent.
use super::{Context, ReconcileError, error_policy, session::State};
use crate::{
    kubernetes::{Error, resources::*},
    registration::Operation,
};
use futures_util::StreamExt;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use kube::{
    Api, ResourceExt,
    runtime::{
        controller::{Action, Config, Controller as KubeController},
        watcher,
    },
};
use std::sync::Arc;

/// Reconcile dispatch intent when node resources change.
pub(super) async fn run(api: Api<SuntitleNode>, context: Arc<Context>) {
    KubeController::new(api, watcher::Config::default())
        .with_config(Config::default().concurrency(1))
        .run(reconcile, error_policy, context)
        .for_each(|result| async move {
            if let Err(error) = result {
                eprintln!("spec controller: {error}");
            }
        })
        .await;
}

/// Reconcile dispatch intent for a node revision.
async fn reconcile(node: Arc<SuntitleNode>, ctx: Arc<Context>) -> Result<Action, ReconcileError> {
    ctx.state
        .lock()
        .await
        .reconcile_spec(&node)
        .await
        .map_err(ReconcileError::Failed)?;
    Ok(Action::await_change())
}

/// Choose apply or removal from node deletion state.
pub(super) fn operation(node: &SuntitleNode) -> Operation {
    if node.metadata.deletion_timestamp.is_some() {
        Operation::Remove
    } else {
        Operation::Apply
    }
}

impl State {

    /// Record intent only for a new generation or deletion.
    async fn reconcile_spec(&mut self, node: &SuntitleNode) -> Result<(), Error> {
        let generation = node.metadata.generation.unwrap();
        let operation = operation(node);
        if node
            .status
            .as_ref()
            .and_then(|status| status.dispatch.as_ref())
            .is_some_and(|dispatch| {
                dispatch.generation == generation && dispatch.operation == operation
            })
        {
            return Ok(());
        }
        let mut node = node.clone();
        if operation == Operation::Remove {
            if !node.finalizers().iter().any(|f| f == NODE_FINALIZER) {
                return Ok(());
            }
        } else {
            let host = self.hosts_api.get(&node.spec.host_ref.name).await?;
            if host.metadata.deletion_timestamp.is_some() {
                return Ok(());
            }
            if let Some(input) = &node.spec.input_ref {
                let upstream = self.nodes_api.get(&input.name).await?;
                if upstream.metadata.deletion_timestamp.is_some()
                    || !upstream.spec.enabled
                    || upstream.uid() == node.uid()
                    || !compatible(&upstream.spec.settings, &node.spec.settings)
                {
                    return self
                        .invalid(
                            &mut node,
                            "InvalidInput",
                            "Input MUST reference an available node with a compatible output.",
                        )
                        .await;
                }
            }
        }
        let previous = node.status.clone();
        node.status.get_or_insert_default().dispatch = Some(DispatchStatus {
            generation,
            operation: self::operation(&node),
            operation_id: uuid::Uuid::new_v4().to_string(),
            session_id: None,
            success: None,
        });
        self.node_status(&mut node, &previous).await
    }
}

impl State {

    /// Record a validation failure in node status.
    async fn invalid(
        &mut self,
        node: &mut SuntitleNode,
        reason: &str,
        message: &str,
    ) -> Result<(), Error> {
        let previous = node.status.clone();
        let status = node.status.get_or_insert_default();
        status.phase = Some(NodePhase::Failed);
        status.message = Some(message.into());
        status.conditions.push(Condition {
            last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
            observed_generation: Some(node.metadata.generation.unwrap_or(1)),
            type_: "Updated".into(),
            status: "False".into(),
            reason: (reason).into(),
            message: (message).into(),
        });
        status.conditions.push(Condition {
            last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
            observed_generation: Some(node.metadata.generation.unwrap_or(1)),
            type_: "Ready".into(),
            status: "False".into(),
            reason: (reason).into(),
            message: (message).into(),
        });
        self.node_status(node, &previous).await
    }
}

/// Check whether an upstream output suits the downstream node.
fn compatible(input: &NodeSettings, output: &NodeSettings) -> bool {
    matches!(
        (input, output),
        (NodeSettings::AudioSource(_), NodeSettings::Transcriber(_))
            | (
                NodeSettings::Transcriber(_),
                NodeSettings::Translator(_) | NodeSettings::Display(_)
            )
            | (NodeSettings::Translator(_), NodeSettings::Display(_))
    )
}
