//! Generation/deletion-driven dispatch intent.
//! Pipeline input references MUST NOT gate installation or runnable startup.
use super::{Context, ReconcileError, error_policy, plan::Plan, session::State};
use futures_util::StreamExt;
use kube::{
    Api, ResourceExt,
    runtime::{
        controller::{Action, Config, Controller as KubeController},
        watcher,
    },
};
use std::sync::Arc;
use suntitle_core::{
    kubernetes::{Error, resources::*},
    registration::Operation,
};

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

impl State {
    /// Record intent only for a new generation or deletion.
    pub(super) async fn reconcile_spec(&mut self, node: &SuntitleNode) -> Result<(), Error> {
        let Some(plan) = Plan::for_node(node) else {
            return Ok(());
        };
        let generation = node.metadata.generation.unwrap();
        if node
            .status
            .as_ref()
            .and_then(|status| status.dispatch.as_ref())
            .is_some_and(|dispatch| plan.matches(node, dispatch))
        {
            return Ok(());
        }
        let mut node = node.clone();
        if plan.is_removal() {
            if !node.finalizers().iter().any(|f| f == NODE_FINALIZER) {
                return Ok(());
            }
        } else {
            let host = self.hosts_api.get(&node.spec.host_ref.name).await?;
            if host.metadata.deletion_timestamp.is_some() {
                return Ok(());
            }
        }
        let previous = node.status.clone();
        let artifact_reference = plan.artifact_reference().map(str::to_owned);
        if plan.operation() == Operation::Download {
            for condition in &mut node.status.get_or_insert_default().conditions {
                if condition.type_ == "Installed" {
                    condition.status = "False".into();
                    condition.reason = "Preparing".into();
                    condition.message =
                        "Waiting for host installation of the selected artifact.".into();
                }
            }
        }
        node.status.get_or_insert_default().dispatch = Some(DispatchStatus {
            artifact_reference,
            generation,
            operation: plan.operation(),
            operation_id: uuid::Uuid::new_v4().to_string(),
            session_id: None,
            success: None,
        });
        self.node_status(&mut node, &previous).await
    }
}
