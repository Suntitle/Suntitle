//! Extensions progress from Preparing to ArtifactPublished or PreparationFailed.
//! Publication precedes node resolution, host download, installation, and execution.
//! Terminal failures may defer a new attempt under bounded controller backoff.
mod archive;
mod executor;
mod git;
mod identity;
mod sources;
pub(super) use sources::Sources;
#[cfg(test)]
mod tests;

use crate::kubernetes::{
    Error,
    resources::{ExtensionStatus, SuntitleExtension},
};
use executor::{Executor, PublishExtension, TaskStatus};
use futures_util::StreamExt;
use identity::ExecutionId;
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
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, mpsc};

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
    /// Background executor with short in-memory status lookups.
    executor: Arc<Executor>,
    /// Desired destination and independent transfer limits.
    config: PublicationConfig,
    /// Retry deadlines survive repeated watch events within this process.
    retries: Mutex<HashMap<ExecutionId, Instant>>,
}

/// Run independent resource reconciliation and Apalis workers until shutdown.
pub(super) async fn run(
    api: Api<SuntitleExtension>,
    config: PublicationConfig,
    cancellation: CancellationToken,
    sources: Sources,
) {
    let (notify, receiver) = mpsc::unbounded_channel::<ObjectRef<SuntitleExtension>>();
    let (executor, mut worker) =
        Executor::start(sources, config.concurrency, cancellation.clone(), notify);
    let context = Arc::new(PublicationContext {
        api: api.clone(),
        executor,
        config,
        retries: Mutex::new(HashMap::new()),
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

/// Observe desired state, accept missing work, or publish an already verified result.
async fn reconcile(
    extension: Arc<SuntitleExtension>,
    ctx: Arc<PublicationContext>,
) -> Result<Action, Error> {
    if extension.metadata.deletion_timestamp.is_some() {
        return Ok(Action::await_change());
    }
    let desired = ExecutionId::for_extension(&extension, &ctx.config)?;
    let current = extension.status.as_ref();
    let id = current
        .and_then(|status| status.task_id.as_deref())
        .and_then(ExecutionId::parse)
        .filter(|id| id.matches(&desired));
    let Some(id) = id else {
        return enqueue(&extension, &ctx, desired).await;
    };
    if current.is_some_and(|status| {
        status.observed_generation == extension.metadata.generation
            && status.artifact_reference.is_some()
            && status.conditions.iter().any(|condition| {
                condition.type_ == "Ready"
                    && condition.status == "True"
                    && condition.observed_generation == extension.metadata.generation
            })
    }) {
        return Ok(Action::await_change());
    }
    match ctx.executor.status(&id).await? {
        Some(TaskStatus::Queued | TaskStatus::Running) => Ok(Action::await_change()),
        Some(TaskStatus::Succeeded { reference }) => {
            write_status(
                &ctx.api,
                &extension,
                PublicationOutcome::Published {
                    task: id.clone(),
                    reference,
                },
            )
            .await?;
            ctx.retries.lock().await.remove(&id);
            Ok(Action::await_change())
        }
        Some(TaskStatus::Failed { error }) => {
            write_status(
                &ctx.api,
                &extension,
                PublicationOutcome::Failed {
                    task: id.clone(),
                    error,
                },
            )
            .await?;
            if id.attempt() >= 3 {
                return Ok(Action::await_change());
            }
            let deadline = *ctx
                .retries
                .lock()
                .await
                .entry(id.clone())
                .or_insert_with(|| Instant::now() + Duration::from_secs(5 << id.attempt()));
            if let Some(delay) = deadline.checked_duration_since(Instant::now()) {
                return Ok(Action::requeue(delay));
            }
            ctx.retries.lock().await.remove(&id);
            enqueue(&extension, &ctx, id.next_attempt()).await
        }
        None => enqueue(&extension, &ctx, id).await,
    }
}

/// Enqueue before persisting identity; completion notifications retain the early-result race.
async fn enqueue(
    extension: &SuntitleExtension,
    ctx: &PublicationContext,
    id: ExecutionId,
) -> Result<Action, Error> {
    let destination = id.destination(&ctx.config.repository, &extension.name_any());
    let id = ctx
        .executor
        .enqueue(PublishExtension {
            id,
            key: ObjectRef::from_obj(extension),
            spec: extension.spec.clone(),
            destination,
        })
        .await?;
    write_status(
        &ctx.api,
        extension,
        PublicationOutcome::Preparing { task: id },
    )
    .await?;
    Ok(Action::await_change())
}

/// Publication state from which Kubernetes status fields are derived together.
enum PublicationOutcome {
    /// Accepted work awaiting completion.
    Preparing {
        /// Current preparation attempt.
        task: ExecutionId,
    },
    /// Complete artifact verified through its digest reference.
    Published {
        /// Completed preparation attempt.
        task: ExecutionId,
        /// Canonical artifact pull reference.
        reference: String,
    },
    /// Terminal attempt awaiting a possible controller retry.
    Failed {
        /// Failed preparation attempt.
        task: ExecutionId,
        /// Credential-free explanation suitable for resource status.
        error: String,
    },
}

/// Patch owned status fields only when the resource snapshot is still current.
async fn write_status(
    api: &Api<SuntitleExtension>,
    extension: &SuntitleExtension,
    outcome: PublicationOutcome,
) -> Result<(), Error> {
    let (task, reference, ready, reason, message) = match outcome {
        PublicationOutcome::Preparing { task } => (
            task,
            None,
            false,
            "Preparing",
            "Source preparation and registry publication are queued.".to_string(),
        ),
        PublicationOutcome::Published { task, reference } => (
            task,
            Some(reference),
            true,
            "ArtifactPublished",
            "Prepared extension artifact is available in the registry.".to_string(),
        ),
        PublicationOutcome::Failed { task, error } => {
            (task, None, false, "PreparationFailed", error)
        }
    };
    let previous = extension
        .status
        .as_ref()
        .map(|status| status.conditions.as_slice())
        .unwrap_or_default();
    let mut status = ExtensionStatus {
        observed_generation: extension.metadata.generation,
        task_id: Some(task.encode()),
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
    api.patch_status(&extension.name_any(), &PatchParams::default(), &Patch::Merge(serde_json::json!({ "metadata": {"uid": extension.uid(), "resourceVersion": extension.resource_version()}, "status": {"observedGeneration": status.observed_generation, "taskId": status.task_id, "artifactReference": status.artifact_reference, "conditions": status.conditions} }))).await?;
    Ok(())
}
