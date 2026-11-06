//! Extensions progress from Preparing to ArtifactPublished or PreparationFailed.
//! Publication precedes node resolution, host download, installation, and execution.
//! Apalis retries failed tasks up to the specification limit; only terminal outcomes wake reconciliation.
mod archive;
mod executor;
mod git;
mod identity;
mod sources;
pub(super) use executor::{PublishExtension, storage};
pub(super) use sources::Sources;
#[cfg(test)]
mod tests;

use apalis::prelude::*;
use apalis_sqlite::SqliteStorage;
use futures_util::StreamExt;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use kube::{
    Api, ResourceExt,
    api::{Patch, PatchParams},
    runtime::{
        controller::{Action, Config, Controller},
        reflector::ObjectRef,
        watcher,
    },
};
use oci_artifact::{CancellationToken, RegistryAuth, client::TransportOptions};
use std::{sync::Arc, time::Duration};
use suntitle_core::kubernetes::{
    Error,
    resources::{ExtensionStatus, SuntitleExtension},
};
use tokio::sync::mpsc;

/// Destination and resource limits for automatic extension publication.
#[derive(Clone)]
pub struct PublicationConfig {
    /// Reachable registry authority and repository prefix, without a scheme or tag.
    pub repository: String,
    /// Explicit destination authentication, never serialized into status or IDs.
    pub authentication: RegistryAuth,
    /// Credentials keyed by source authority; secrets MUST NOT cross authorities.
    pub source_authentication: std::collections::BTreeMap<String, RegistryAuth>,
    /// TLS trust and explicitly enabled development HTTP.
    pub transport: TransportOptions,
    /// Number of concurrently active publication jobs.
    pub concurrency: usize,
    /// Maximum wall time for one source preparation and publication attempt.
    pub task_timeout: Duration,
}
impl PublicationConfig {
    /// Validate the repository and limits before accepting any work.
    pub fn validate(&self) -> Result<(), Error> {
        if self.concurrency == 0
            || self.task_timeout.is_zero()
            || self.repository.contains("://")
            || self.repository.contains('@')
        {
            return Err("invalid extension publication configuration".into());
        }
        oci_artifact::Publisher::validate_repository(&self.repository)?;
        Ok(())
    }
}
/// Explicit authority-scoped credentials loaded from a private configuration file.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryCredential {
    /// Registry or source account name.
    pub username: String,
    /// Password or access token. MUST NOT be logged or copied into resource status.
    pub password: String,
}
impl RegistryCredential {
    /// Build transport credentials without serializing them into managed state.
    pub fn authentication(&self) -> RegistryAuth {
        RegistryAuth::Basic(self.username.clone(), self.password.clone())
    }
}
/// Controller-local dependencies. Transfer workers never lock host session state.
struct PublicationContext {
    /// Resource API supporting status writes guarded by UID and resource version.
    api: Api<SuntitleExtension>,
    /// Apalis owns queue admission, deduplication, task state, and terminal results.
    storage: SqliteStorage<PublishExtension>,
    /// Desired destination and independent transfer limits.
    config: PublicationConfig,
}

/// Run resource reconciliation and independent publication tasks until shutdown.
pub(super) async fn run(
    api: Api<SuntitleExtension>,
    config: PublicationConfig,
    cancellation: CancellationToken,
    sources: Sources,
    storage: SqliteStorage<PublishExtension>,
) {
    let (notify, receiver) = mpsc::unbounded_channel::<ObjectRef<SuntitleExtension>>();
    let mut worker = executor::start(
        storage.clone(),
        sources,
        config.concurrency,
        cancellation.clone(),
        notify,
    );
    let context = Arc::new(PublicationContext {
        api: api.clone(),
        storage,
        config,
    });
    let controller = Controller::new(api, watcher::Config::default())
        .with_config(Config::default().concurrency(16))
        .reconcile_on(tokio_stream::wrappers::UnboundedReceiverStream::new(
            receiver,
        ))
        .run(
            |extension, ctx| async move {
                reconcile(extension, ctx)
                    .await
                    .map_err(super::ReconcileError::Failed)
            },
            |_, error: &super::ReconcileError, _| {
                eprintln!("extension publication reconcile: {error}");
                Action::requeue(Duration::from_secs(5))
            },
            context,
        )
        .for_each(|result| async {
            if let Err(error) = result {
                eprintln!("extension publication controller: {error}");
            }
        });
    tokio::pin!(controller);
    tokio::select! { _ = &mut controller => {}, _ = cancellation.cancelled() => {}, result = &mut worker => { eprintln!("extension publication worker exited: {result:?}"); return; } }
    cancellation.cancel();
    let _ = worker.await;
}

/// Retain published artifacts for the current generation; otherwise submit and observe Apalis work.
async fn reconcile(
    extension: Arc<SuntitleExtension>,
    ctx: Arc<PublicationContext>,
) -> Result<Action, Error> {
    if let Some(_deleted_at) = &extension.metadata.deletion_timestamp {
        return Ok(Action::await_change());
    }
    if let Some(status) = &extension.status
        && let Some(_reference) = &status.artifact_reference
        && status.observed_generation == extension.metadata.generation
    {
        return Ok(Action::await_change());
    }
    let id = identity::task_id(&extension, &ctx.config)?;
    submit(&extension, &ctx, id.clone()).await?;
    let state = ctx.storage.clone().check_status([id.clone()]).await?.pop();
    write_status(&ctx.api, &extension, &id, &state).await?;
    Ok(Action::await_change())
}

/// Submit immutable source inputs without waiting for execution or capacity.
async fn submit(
    extension: &SuntitleExtension,
    ctx: &PublicationContext,
    id: TaskId,
) -> Result<(), Error> {
    let task = TaskBuilder::new(PublishExtension {
        destination: format!(
            "{}/{}:publication-{id}",
            ctx.config.repository.trim_end_matches('/'),
            extension.name_any()
        ),
        namespace: extension.namespace(),
        name: extension.name_any(),
        spec: extension.spec.clone(),
    })
    .task_id(id.clone())
    .idempotency_key(id.to_string())
    .max_attempts(extension.spec.max_attempts as usize)
    .build();
    ctx.storage.clone().push_task(task).await?;
    Ok(())
}

/// Derive publication status directly from SQLite's stored result.
/// The patch MUST retain UID/resource-version preconditions to reject obsolete results.
async fn write_status(
    api: &Api<SuntitleExtension>,
    extension: &SuntitleExtension,
    task: &TaskId,
    state: &Option<TaskResult<String>>,
) -> Result<(), Error> {
    let (reference, ready, reason, message) = match state.as_ref().map(|task| &task.result) {
        None => (
            None,
            false,
            "Preparing",
            "Source preparation and registry publication are queued, running, or retrying."
                .to_string(),
        ),
        Some(Ok(reference)) => (
            Some(reference.clone()),
            true,
            "ArtifactPublished",
            "Prepared extension artifact is available in the registry.".to_string(),
        ),
        Some(Err(error)) => (None, false, "PreparationFailed", error.clone()),
    };
    let previous = extension
        .status
        .as_ref()
        .map(|status| status.conditions.as_slice())
        .unwrap_or_default();
    let mut status = ExtensionStatus {
        observed_generation: extension.metadata.generation,
        task_id: Some(task.to_string()),
        attempts: state.as_ref().map(|task| task.attempt),
        artifact_reference: reference,
        conditions: vec![Condition {
            type_: "Ready".into(),
            status: if ready { "True" } else { "False" }.into(),
            observed_generation: extension.metadata.generation,
            reason: reason.into(),
            message,
            last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
        }],
    };
    super::conditions::merge(previous, &mut status.conditions);
    if serde_json::to_value(&extension.status)? == serde_json::to_value(&status)? {
        return Ok(());
    }

    api.patch_status(&extension.name_any(), &PatchParams::default(), &Patch::Merge(serde_json::json!({ "metadata": {"uid": extension.uid(), "resourceVersion": extension.resource_version()}, "status": {"observedGeneration": status.observed_generation, "taskId": status.task_id, "attempts": status.attempts, "artifactReference": status.artifact_reference, "conditions": status.conditions} }))).await?;
    Ok(())
}
