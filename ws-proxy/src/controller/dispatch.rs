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
        WatchStreamExt,
        controller::{Action, Config, Controller as KubeController},
        reflector, watcher,
    },
};
use std::sync::Arc;

/// Filter spec revisions before reconciling dispatch intent.
pub(super) async fn run(
    api: Api<SuntitleNode>,
    nodes: reflector::Store<SuntitleNode>,
    writer: reflector::store::Writer<SuntitleNode>,
    context: Arc<Context>,
) {
    let stream = watcher(api, watcher::Config::default())
        .default_backoff()
        .reflect(writer)
        .applied_objects()
        .predicate_filter(spec_revision, Default::default());
    KubeController::for_stream(stream, nodes)
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

/// Status-only updates do not change this watch key.
pub(super) fn spec_revision(node: &SuntitleNode) -> Option<u64> {
    Some(
        (node.metadata.generation.unwrap() as u64) << 1
            | u64::from(node.metadata.deletion_timestamp.is_some()),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify dispatch revisions track generation and deletion.
    #[test]
    fn revision_ignores_status_and_metadata_but_includes_deletion() {
        let mut node = super::super::tests::node();
        let original = spec_revision(&node);
        node.status.get_or_insert_default().message = Some("Updated".into());
        node.metadata.resource_version = Some("new".into());
        node.metadata.finalizers = Some(vec![NODE_FINALIZER.into()]);
        assert_eq!(spec_revision(&node), original);
        node.metadata.generation = Some(2);
        let updated = spec_revision(&node);
        assert_ne!(updated, original);
        node.metadata.deletion_timestamp = Some(Time(k8s_openapi::jiff::Timestamp::now()));
        assert_ne!(spec_revision(&node), updated);
        assert_eq!(operation(&node), Operation::Remove);
    }
}
