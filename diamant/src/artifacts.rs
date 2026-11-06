//! Background artifact downloads and installation preparation for the current host session.
//!
//! A single task collection owns current and superseded downloads. Replacement
//! cancels a child token; session shutdown cancels their shared parent and MUST
//! join every task. Tasks MUST NOT be aborted while a renderer can still be active.
//! Successful preparation is handed to the process manager before reporting Installed.
//! Completed payloads are cached by artifact reference. Command changes MUST NOT pull files.
use std::{collections::HashMap, path::PathBuf};

use oci_artifact::{CancellationToken, DownloadRequest, Downloader, client::TransportOptions};
use tokio::task::{Id, JoinSet};

use super::installation::PreparedExtension;
use suntitle_core::{
    kubernetes::adapter::ResolvedAdapter,
    registration::{ApplyIdentity, Operation, RegistrationClientMessage},
};

/// Registry access and persistent payload location.
#[derive(Clone, Debug, Default)]
pub struct ArtifactOptions {
    /// Override the OS local data directory used for completed payloads.
    pub directory: Option<PathBuf>,
    /// Explicitly permit unencrypted registry access.
    pub plain_http: bool,
}

impl ArtifactOptions {
    /// Resolve configured storage without creating it or changing global state.
    pub fn directory(&self) -> std::io::Result<PathBuf> {
        self.directory
            .clone()
            .or_else(|| dirs::data_local_dir().map(|p| p.join("suntitle/diamant/artifacts")))
            .ok_or_else(|| {
                std::io::Error::other("artifact directory unavailable; configure --artifact-dir")
            })
    }
}

/// Current desired operation for one node. Completed identities prevent replay.
struct Job {
    /// Resource revision and host session eligible to report a result.
    identity: ApplyIdentity,
    /// Child token used to cancel this operation when replaced or removed.
    cancellation: CancellationToken,
    /// Signals task exit, including panics, so removal can await cleanup.
    finished: CancellationToken,
    /// Identifies task failures returned by the shared task collection.
    task_id: Id,
    /// Prepared installation awaiting handoff to the process manager.
    installation: Option<PreparedExtension>,
}

/// Task output before stale results are filtered and wire reports are constructed.
struct Completion {
    /// Identity captured when the transfer started.
    identity: ApplyIdentity,
    /// Published payload path or a diagnostic from the failed transfer.
    result: Result<PreparedExtension, String>,
}

impl Completion {
    /// Convert a current completion into the existing acknowledgement contract.
    fn into_report(self) -> RegistrationClientMessage {
        let (success, message) = match self.result {
            Ok(installation) => (
                true,
                format!(
                    "Extension installed at {}; awaiting argument construction and launch.",
                    installation.root.display()
                ),
            ),
            Err(error) => (false, format!("Extension preparation failed: {error}")),
        };
        RegistrationClientMessage::Acknowledgement {
            identity: self.identity,
            operation: Operation::Download,
            success,
            message: Some(message),
        }
    }
}

/// Owns downloads so network transfers do not block registration heartbeats.
pub struct ArtifactDownloads {
    /// Storage root resolved once for this host session.
    directory: PathBuf,
    /// Shared registry client and connection pool cloned into transfers.
    downloader: Downloader,
    /// Current node identities and cancellation signals, including completed jobs.
    jobs: HashMap<String, Job>,
    /// Sole owner of running tasks and their completion results.
    tasks: JoinSet<Completion>,
    /// Parent token covering current and superseded tasks until session shutdown.
    cancellation: CancellationToken,
}

impl ArtifactDownloads {
    /// Construct one registry client and task owner for a host session.
    pub fn new(options: ArtifactOptions) -> Result<Self, oci_artifact::Error> {
        Ok(Self {
            directory: options.directory()?,
            downloader: Downloader::new(TransportOptions {
                plain_http: options.plain_http,
                ..Default::default()
            })?,
            jobs: HashMap::new(),
            tasks: JoinSet::new(),
            cancellation: CancellationToken::new(),
        })
    }

    /// Download and prepare current work, cancelling its predecessor without aborting rendering.
    /// Duplicate and older generations MUST NOT replace the current operation.
    pub fn start(&mut self, identity: ApplyIdentity, reference: String, adapter: ResolvedAdapter) {
        let key = identity.target.uid.clone();
        if self.jobs.get(&key).is_some_and(|job| {
            job.identity == identity || job.identity.generation > identity.generation
        }) {
            return;
        }
        if let Some(job) = self.jobs.remove(&key) {
            job.cancellation.cancel();
        }
        let cancellation = self.cancellation.child_token();
        let token = cancellation.clone();
        let finished = CancellationToken::new();
        let guard = finished.clone().drop_guard();
        let directory = self.directory.clone();
        let downloader = self.downloader.clone();
        let report_identity = identity.clone();
        let task_id = self
            .tasks
            .spawn(async move {
                let _finished = guard;
                let result = async {
                    let payload = download(directory, downloader, &reference, token.clone())
                        .await
                        .map_err(|e| e.to_string())?;
                    if token.is_cancelled() {
                        return Err("extension preparation cancelled".into());
                    }
                    let installation = PreparedExtension::prepare(
                        report_identity.clone(),
                        reference,
                        payload,
                        adapter,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                    if token.is_cancelled() {
                        return Err("extension preparation cancelled".into());
                    }
                    Ok(installation)
                }
                .await;
                Completion {
                    identity: report_identity,
                    result,
                }
            })
            .id();
        self.jobs.insert(
            key,
            Job {
                identity,
                cancellation,
                finished,
                task_id,
                installation: None,
            },
        );
    }

    /// Join completions and report only the current operation for each node.
    /// Waiting on an empty collection stays pending. Cancellation is safe in select.
    pub async fn next_report(&mut self) -> RegistrationClientMessage {
        loop {
            let result = match self.tasks.join_next().await {
                Some(result) => result,
                None => std::future::pending().await,
            };
            let completion = match result {
                Ok(completion) => completion,
                Err(error) => {
                    let Some(job) = self.jobs.values().find(|job| job.task_id == error.id()) else {
                        continue;
                    };
                    Completion {
                        identity: job.identity.clone(),
                        result: Err(format!("download task failed: {error}")),
                    }
                }
            };
            if let Some(job) = self.jobs.get_mut(&completion.identity.target.uid)
                && job.identity == completion.identity
            {
                job.installation = completion.result.as_ref().ok().cloned();
                return completion.into_report();
            }
        }
    }

    /// Move the current prepared installation to the process manager exactly once.
    pub fn take_prepared(&mut self, identity: &ApplyIdentity) -> Option<PreparedExtension> {
        self.jobs
            .get_mut(&identity.target.uid)
            .filter(|job| job.identity == *identity)?
            .installation
            .take()
    }

    /// Cancel the matching node revision and wait until its task exits.
    /// Completed payloads remain available for later installation or cleanup.
    pub async fn remove(&mut self, identity: &ApplyIdentity) {
        if self
            .jobs
            .get(&identity.target.uid)
            .is_some_and(|job| job.identity.generation <= identity.generation)
            && let Some(job) = self.jobs.remove(&identity.target.uid)
        {
            job.cancellation.cancel();
            job.finished.cancelled().await;
        }
    }

    /// Cancel all descendants and join them before releasing session resources.
    pub async fn shutdown(&mut self) {
        self.cancellation.cancel();
        while self.tasks.join_next().await.is_some() {}
        self.jobs.clear();
    }
}

/// Download into private staging and publish only a complete payload directory.
/// The caller MUST await completion after cancellation so rendering can stop.
async fn download(
    directory: PathBuf,
    downloader: Downloader,
    reference: &str,
    cancellation: CancellationToken,
) -> Result<PathBuf, oci_artifact::Error> {
    let base = directory;
    tokio::fs::create_dir_all(&base).await?;
    let base = tokio::fs::canonicalize(base).await?;
    use sha2::{Digest, Sha256};
    let cache = base.join(format!(
        ".payload-{:x}.json",
        Sha256::digest(reference.as_bytes())
    ));
    if let Ok(bytes) = tokio::fs::read(&cache).await
        && let Ok(payload) = serde_json::from_slice::<PathBuf>(&bytes)
        && payload.starts_with(&base)
        && tokio::fs::try_exists(&payload).await?
    {
        return Ok(payload);
    }
    let staging = tempfile::Builder::new()
        .prefix(".artifact-")
        .tempdir_in(&base)?;
    let mut request = DownloadRequest::new(reference, staging.path().join("content"));
    request.cancellation = cancellation.clone();
    let result = downloader.download(request).await?;
    if cancellation.is_cancelled() {
        return Err(oci_artifact::Error::Cancelled);
    }
    let relative_payload = result
        .payload_path
        .strip_prefix(&result.output_path)
        .map_err(|_| {
            oci_artifact::Error::Unsupported(
                "artifact payload is outside its output directory".into(),
            )
        })?;
    let destination = base.join(uuid::Uuid::new_v4().to_string());
    tokio::fs::rename(&result.output_path, &destination).await?;
    let payload = destination.join(relative_payload);
    let marker = tempfile::NamedTempFile::new_in(&base)?;
    tokio::fs::write(
        marker.path(),
        serde_json::to_vec(&payload).map_err(std::io::Error::other)?,
    )
    .await?;
    tokio::fs::rename(marker.path(), &cache).await?;
    Ok(payload)
}
