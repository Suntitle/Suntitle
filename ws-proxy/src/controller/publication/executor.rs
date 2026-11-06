//! Apalis owns Pending -> Running -> Done/Failed transitions and stored results.
//! SQLite retries each task up to the specification limit without blocking reconciliation.
//! Notifications follow SQLite result storage and wake reconciliation without polling timers.
//! Shutdown joins active transfers. The in-memory database is rebuilt from resources after restart.
use super::sources::Sources;
use apalis::{layers::catch_panic::CatchPanicLayer, prelude::*};
use apalis_sqlite::{SqlitePoolOptions, SqliteStorage};
use futures_util::{StreamExt, TryStreamExt};
use kube::runtime::reflector::ObjectRef;
use oci_artifact::CancellationToken;
use std::time::Duration;
use suntitle_core::kubernetes::{
    Error,
    resources::{SuntitleExtension, SuntitleExtensionSpec},
};
use tokio::{sync::mpsc, task::JoinHandle};

/// Immutable inputs for one publication attempt. Credentials remain in the source client.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(in crate::controller) struct PublishExtension {
    /// Namespace of the resource notified after completion.
    pub(super) namespace: Option<String>,
    /// Name of the resource notified after completion.
    pub(super) name: String,
    /// Desired source and adapter snapshot.
    pub(super) spec: SuntitleExtensionSpec,
    /// Destination tag shared by retries of this desired revision.
    pub(super) destination: String,
}

/// Initialize an isolated in-memory database before starting controllers.
pub(in crate::controller) async fn storage() -> Result<SqliteStorage<PublishExtension>, Error> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect(":memory:")
        .await?;
    SqliteStorage::setup(&pool).await?;
    Ok(SqliteStorage::new(&pool))
}

/// Start Apalis and bridge stored completions to resource reconciliation.
/// Concurrency MUST be positive and validated by the caller.
pub(super) fn start(
    storage: SqliteStorage<PublishExtension>,
    sources: Sources,
    concurrency: usize,
    cancellation: CancellationToken,
    notify: mpsc::UnboundedSender<ObjectRef<SuntitleExtension>>,
) -> JoinHandle<()> {
    let (completed, receiver) = mpsc::unbounded_channel();
    let worker = WorkerBuilder::new("extension-publication")
        .backend(
            storage
                .clone()
                .poll_with_interval(Duration::from_millis(100)),
        )
        .concurrency(concurrency)
        .data(sources)
        // Ordinary errors preserve SQLite retries; the default panic handler aborts the task.
        // The callback MUST accept any payload lifetime for spawned workers.
        .layer(CatchPanicLayer::with_panic_handler(
            (|_| std::io::Error::other("publication worker panicked"))
                as for<'panic> fn(&Box<dyn std::any::Any + Send + 'panic>) -> std::io::Error,
        ))
        .build(
            move |task: PublishExtension, id: TaskId, attempt: Attempt, sources: Data<Sources>| {
                // One observer follows the task across storage-managed retries.
                if attempt.current() == 1 {
                    let mut key = ObjectRef::new(&task.name);
                    key.namespace = task.namespace.clone();
                    let _ = completed.send((id, key));
                }
                async move { sources.publish(&task).await.map_err(std::io::Error::other) }
            },
        );
    let observer_notify = notify.clone();
    let completions = tokio_stream::wrappers::UnboundedReceiverStream::new(receiver)
        .for_each_concurrent(None, move |(id, key)| {
            let mut storage = storage.clone();
            let notify = observer_notify.clone();
            async move {
                loop {
                    let result: Result<Option<TaskResult<String>>, _> =
                        storage.wait_for_single(id.clone()).try_next().await;
                    if let Ok(Some(_)) = result {
                        let _ = notify.send(key);
                        break;
                    }
                    eprintln!("extension completion observation: {result:?}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        });
    tokio::spawn(async move {
        let shutdown = cancellation.clone();
        let execution = worker.run_until(async move {
            tokio::select! {
                _ = shutdown.cancelled() => {},
                _ = notify.closed() => {},
            }
            shutdown.cancel();
            Ok::<(), WorkerError>(())
        });
        tokio::pin!(execution, completions);
        tokio::select! {
            result = &mut execution => {
                if let Err(error) = result { eprintln!("extension worker stopped: {error}"); }
            }
            _ = &mut completions => {},
        }
        cancellation.cancel();
    })
}
