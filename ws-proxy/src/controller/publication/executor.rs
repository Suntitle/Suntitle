//! Attempts transition Queued -> Running -> Succeeded/Failed before notification.
//! Controller retries create new identities. Lost tasks are reconstructed after restart.
use super::{identity::ExecutionId, sources::Sources};
use crate::kubernetes::{Error, resources::SuntitleExtensionSpec};
use apalis::prelude::*;
use futures_util::{FutureExt, SinkExt, StreamExt};
use std::{
    collections::HashMap,
    panic::AssertUnwindSafe,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock, mpsc};

/// Queryable outcome independent of Apalis acknowledgement state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TaskStatus {
    /// Accepted by the executor and awaiting a worker.
    Queued,
    /// Source preparation or registry publication is active.
    Running,
    /// Verified canonical registry reference.
    Succeeded { reference: String },
    /// Terminal attempt error. The controller owns retries.
    Failed { error: String },
}
/// Reconstructible source and destination for one resource revision.
#[derive(Clone)]
pub(super) struct PublishExtension {
    /// Attempt identity including UID, specification hash, destination, and attempt.
    pub id: ExecutionId,
    /// Namespaced resource key for controller notifications.
    pub key: kube::runtime::reflector::ObjectRef<crate::kubernetes::resources::SuntitleExtension>,
    /// Desired source and adapter snapshot.
    pub spec: SuntitleExtensionSpec,
    /// Immutable destination tag unique to this attempt.
    pub destination: String,
}
/// Retained attempt state and conservative observed-result expiration.
struct Record {
    /// Current lifecycle state. Terminal states MUST NOT regress.
    status: TaskStatus,
    /// First terminal observation; unobserved results MUST be retained.
    observed: Option<Instant>,
}
/// Apalis-backed in-memory executor and terminal outcome registry.
pub(super) struct Executor {
    /// Serialize submissions to make duplicate acceptance atomic.
    sink: Mutex<MemorySink<PublishExtension>>,
    /// Short-lived locks; source and registry work never hold them.
    records: Arc<RwLock<HashMap<ExecutionId, Record>>>,
}
impl Executor {
    /// Start bounded-concurrency workers with Apalis retries disabled.
    pub fn start(
        sources: Sources,
        concurrency: usize,
        cancellation: oci_artifact::CancellationToken,
        notify: mpsc::UnboundedSender<
            kube::runtime::reflector::ObjectRef<crate::kubernetes::resources::SuntitleExtension>,
        >,
    ) -> (Arc<Self>, tokio::task::JoinHandle<()>) {
        let (tx, rx) = futures_channel::mpsc::unbounded::<Task<PublishExtension>>();
        let sink = MemorySink::new(Arc::new(futures_util::lock::Mutex::new(Box::new(
            tx.sink_map_err(|e| MemoryStorageError::Other(Box::new(e))),
        ))));
        let storage = MemoryStorage::new_with(sink.clone(), rx.boxed());
        let executor = Arc::new(Self {
            sink: Mutex::new(sink),
            records: Arc::new(RwLock::new(HashMap::new())),
        });
        let records = executor.records.clone();
        let task = tokio::spawn(async move {
            let worker_context = WorkerContext::new("extension-publication");
            let worker = WorkerBuilder::new(worker_context.clone())
                .backend(storage)
                .concurrency(concurrency)
                .data(WorkerData {
                    sources,
                    records,
                    notify,
                })
                .build(execute);
            let execution = worker.run();
            tokio::pin!(execution);
            tokio::select! {
                result = &mut execution => { if let Err(error) = result { eprintln!("extension worker stopped: {error}"); } },
                _ = cancellation.cancelled() => { let _ = worker_context.stop(); let _ = execution.await; }
            }
        });
        (executor, task)
    }
}
impl Executor {
    /// Coalesce concurrent duplicate submissions and roll back failed acceptance.
    pub async fn enqueue(&self, task: PublishExtension) -> Result<ExecutionId, Error> {
        let mut sink = self.sink.lock().await;
        {
            let mut records = self.records.write().await;
            records.retain(|_, record| {
                record
                    .observed
                    .is_none_or(|at| at.elapsed() < Duration::from_secs(3600))
            });
            if records.contains_key(&task.id) {
                return Ok(task.id);
            }
            if records.len() >= 4096 {
                return Err("extension task capacity exceeded".into());
            }
            records.insert(
                task.id.clone(),
                Record {
                    status: TaskStatus::Queued,
                    observed: None,
                },
            );
        }
        let id = task.id.clone();
        if let Err(error) = sink.send(Task::new(task)).await {
            self.records.write().await.remove(&id);
            return Err(Box::new(error));
        }
        Ok(id)
    }
    /// Terminal observations start conservative retention without removing results.
    pub async fn status(&self, id: &ExecutionId) -> Result<Option<TaskStatus>, Error> {
        let mut records = self.records.write().await;
        Ok(records.get_mut(id).map(|record| {
            if matches!(
                record.status,
                TaskStatus::Succeeded { .. } | TaskStatus::Failed { .. }
            ) {
                record.observed.get_or_insert_with(Instant::now);
            }
            record.status.clone()
        }))
    }
}

/// Shared worker dependencies injected without coupling the controller to Apalis.
#[derive(Clone)]
struct WorkerData {
    /// Pooled source and OCI clients.
    sources: Sources,
    /// Queryable attempt results.
    records: Arc<RwLock<HashMap<ExecutionId, Record>>>,
    /// Reliable unbounded completion channel; each accepted attempt emits once.
    notify: mpsc::UnboundedSender<
        kube::runtime::reflector::ObjectRef<crate::kubernetes::resources::SuntitleExtension>,
    >,
}
/// Store terminal outcomes before notifying, including caught worker panics.
async fn execute(task: PublishExtension, data: Data<WorkerData>) {
    {
        let mut records = data.records.write().await;
        if let Some(record) = records.get_mut(&task.id) {
            record.status = TaskStatus::Running;
        }
    }
    let outcome = AssertUnwindSafe(data.sources.publish(&task))
        .catch_unwind()
        .await;
    let status = match outcome {
        Ok(Ok(reference)) => TaskStatus::Succeeded { reference },
        Ok(Err(error)) => TaskStatus::Failed { error },
        Err(_) => TaskStatus::Failed {
            error: "publication worker panicked".into(),
        },
    };
    {
        let mut records = data.records.write().await;
        if let Some(record) = records.get_mut(&task.id) {
            record.status = status;
        }
    }
    // Apalis receives success because the controller owns terminal failure retries.
    let _ = data.notify.send(task.key);
}
