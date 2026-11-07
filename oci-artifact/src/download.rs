//! Download and restore file and directory artifacts.
use std::{
    path::{Component, Path, PathBuf},
    time::Duration,
};

use backon::{ExponentialBuilder, Retryable};
use futures_util::TryStreamExt;
use oci_client::{
    Client, Reference,
    annotations::ORG_OPENCONTAINERS_IMAGE_TITLE as ANNOTATION_TITLE,
    errors::OciDistributionError,
    manifest::{IMAGE_LAYER_GZIP_MEDIA_TYPE, OciDescriptor, OciImageManifest, OciManifest},
    secrets::RegistryAuth,
};
use tokio::io::AsyncWriteExt;

use crate::{CancellationToken, Error, client::TransportOptions, constants::ANNOTATION_UNPACK};

/// Registry source, local output, and caller-supplied credentials.
pub struct DownloadRequest {
    /// Image reference by tag or digest.
    pub reference: String,
    /// Output directory. Existing entries are overwritten in place.
    pub destination: PathBuf,
    /// Explicit registry credentials. Defaults to anonymous access.
    pub authentication: RegistryAuth,
    /// Signal cancellation, then await the download until it returns.
    pub cancellation: CancellationToken,
}

impl DownloadRequest {
    /// Create an anonymous download to a local directory.
    pub fn new(reference: impl Into<String>, destination: impl Into<PathBuf>) -> Self {
        Self {
            reference: reference.into(),
            destination: destination.into(),
            authentication: RegistryAuth::Anonymous,
            cancellation: CancellationToken::new(),
        }
    }
}

/// Completed artifact download.
#[derive(Debug)]
pub struct DownloadResult {
    /// Immutable digest of the downloaded manifest.
    pub manifest_digest: String,
    /// Absolute path of the completed output directory.
    pub output_path: PathBuf,
    /// Restored file or directory identified by the layer filename annotation.
    pub payload_path: PathBuf,
}

/// Registry client for downloading file and directory artifacts.
#[derive(Clone)]
pub struct Downloader {
    client: Client,
}

impl Downloader {
    /// Construct a registry client with the supplied transport settings.
    pub fn new(options: TransportOptions) -> Result<Self, Error> {
        Ok(Self {
            client: Client::try_from(options.build_config()?)?,
        })
    }

    /// Download one named file or unpack a directory with archived permissions.
    ///
    /// A directory archive already contains its prefix. Extraction restores it
    /// below the destination without adding or removing a prefix.
    /// The blob is verified before restoring directly into the destination.
    /// Extraction or local I/O failures may leave partial updates.
    /// Cancellation MUST use the request token. Callers MUST await completion;
    /// dropping the future cannot wait for the renderer to stop.
    pub async fn download(&self, request: DownloadRequest) -> Result<DownloadResult, Error> {
        let reference: Reference = request.reference.parse()?;
        let resolve = (|| {
            self.client
                .pull_manifest(&reference, &request.authentication)
        })
        .retry(retry_policy())
        .when(transient);
        let (manifest, manifest_digest) = tokio::select! {
            biased;
            _ = request.cancellation.cancelled() => return Err(Error::Cancelled),
            result = resolve => result?,
        };
        let manifest = match manifest {
            OciManifest::Image(manifest) => manifest,
            OciManifest::ImageIndex(_) => {
                return Err(Error::Unsupported(
                    "expected an artifact manifest, not an image index".into(),
                ));
            }
        };
        let artifact = artifact_layer(&manifest)?;
        let output_path = std::path::absolute(&request.destination)?;
        let staging = tempfile::tempdir()?;
        let blob = tokio::select! {
            biased;
            _ = request.cancellation.cancelled() => return Err(Error::Cancelled),
            result = self.download_layer(&reference, artifact.descriptor, staging.path()) => result?,
        };
        if request.cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }

        match artifact.restoration {
            Restoration::File { name } => {
                restore_file(
                    &blob,
                    &output_path,
                    name,
                    annotation(artifact.descriptor, crate::constants::ANNOTATION_EXECUTABLE)
                        == Some("true"),
                )
                .await?
            }
            Restoration::Directory => {
                restore_directory(&blob, &output_path, &artifact.descriptor.media_type).await?;
            }
        }

        if request.cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let payload_path = output_path.join(
            annotation(artifact.descriptor, ANNOTATION_TITLE)
                .expect("artifact layer filename was validated"),
        );
        Ok(DownloadResult {
            payload_path,
            manifest_digest,
            output_path,
        })
    }

    /// Save and verify a layer. Each transfer retry uses a fresh private file.
    async fn download_layer(
        &self,
        reference: &Reference,
        layer: &OciDescriptor,
        directory: &std::path::Path,
    ) -> Result<tempfile::TempPath, Error> {
        let transfer = || async {
            let file = tempfile::NamedTempFile::new_in(directory)?;
            let (file, path) = file.into_parts();
            let mut writer = tokio::fs::File::from_std(file);
            let mut written = 0;
            let mut response = self.client.pull_blob_stream(reference, layer).await?;

            // The client verifies the descriptor digest when the stream ends.
            while let Some(chunk) = response.stream.try_next().await.map_err(transfer_error)? {
                writer.write_all(&chunk).await?;
                written += chunk.len() as u64;
                if written > layer.size as u64 {
                    return Err(Error::Verification("blob exceeds descriptor size".into()));
                }
            }
            writer.flush().await?;
            writer.shutdown().await?;
            if written != layer.size as u64 {
                return Err(Error::Verification("blob size mismatch".into()));
            }
            drop(writer);

            Ok(path)
        };

        transfer
            .retry(retry_policy())
            .when(|error| match error {
                Error::Registry(error) => transient(error),
                Error::Transfer(error) => error.kind() != std::io::ErrorKind::InvalidData,
                _ => false,
            })
            .await
    }
}

/// Bound retries for transient registry failures.
fn retry_policy() -> ExponentialBuilder {
    ExponentialBuilder::default()
        .with_min_delay(Duration::from_millis(100))
        .with_max_times(4)
}

/// Retry transport failures, throttling, and server errors.
fn transient(error: &OciDistributionError) -> bool {
    match error {
        OciDistributionError::ServerError { code, .. } => {
            *code == 408 || *code == 429 || *code >= 500
        }
        OciDistributionError::RequestError(error) => {
            error.is_timeout()
                || error.is_connect()
                || error.is_body()
                || error
                    .status()
                    .is_some_and(|status| status.as_u16() == 429 || status.is_server_error())
        }
        _ => false,
    }
}

enum Restoration<'a> {
    File { name: &'a str },
    Directory,
}

struct ArtifactLayer<'a> {
    descriptor: &'a OciDescriptor,
    restoration: Restoration<'a>,
}

/// Accept one named file or unpackable gzip directory layer.
fn artifact_layer(manifest: &OciImageManifest) -> Result<ArtifactLayer<'_>, Error> {
    let [layer] = manifest.layers.as_slice() else {
        return Err(Error::Unsupported(
            "expected one file or directory layer".into(),
        ));
    };
    let title = annotation(layer, ANNOTATION_TITLE)
        .ok_or_else(|| Error::Unsupported("missing layer filename".into()))?;
    if title.is_empty()
        || !Path::new(title)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(Error::Unsupported("invalid layer filename".into()));
    }
    if annotation(&manifest.config, ANNOTATION_TITLE).is_some() {
        return Err(Error::Unsupported("named config is not supported".into()));
    }
    if layer.size < 0 {
        return Err(Error::Unsupported("negative layer size".into()));
    }
    let restoration = if annotation(layer, ANNOTATION_UNPACK) == Some("true") {
        if layer.media_type != IMAGE_LAYER_GZIP_MEDIA_TYPE {
            return Err(Error::Unsupported(
                "expected a gzip directory layer for unpacking".into(),
            ));
        }
        Restoration::Directory
    } else {
        Restoration::File { name: title }
    };

    Ok(ArtifactLayer {
        descriptor: layer,
        restoration,
    })
}

async fn restore_file(blob: &Path, tree: &Path, name: &str, executable: bool) -> Result<(), Error> {
    let path = tree.join(name);
    tokio::fs::create_dir_all(path.parent().unwrap()).await?;
    let mut source = tokio::fs::File::open(blob).await?;
    match tokio::fs::remove_file(&path).await {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let mut output = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await?;
    tokio::io::copy(&mut source, &mut output).await?;
    output.flush().await?;
    output.shutdown().await?;
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        output
            .set_permissions(std::fs::Permissions::from_mode(0o755))
            .await?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    Ok(())
}

#[cfg(unix)]
async fn restore_directory(blob: &Path, tree: &Path, media_type: &str) -> Result<(), Error> {
    let packer = ocirender::StreamingPacker::new(
        vec![ocirender::LayerMeta {
            index: 0,
            media_type: media_type.into(),
        }],
        ocirender::ImageSpec::Dir { path: tree.into() },
        None,
    );
    let ready = packer.notify_layer_ready(0, blob.to_path_buf()).await;
    let rendered = packer.finish().await;
    rendered?;
    ready?;
    Ok(())
}

/// Restore the verified gzip layer on Windows without the Unix-only renderer.
#[cfg(windows)]
async fn restore_directory(blob: &Path, tree: &Path, _media_type: &str) -> Result<(), Error> {
    let (blob, tree) = (blob.to_path_buf(), tree.to_path_buf());
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&tree)?;
        let mut archive =
            tar::Archive::new(flate2::read::GzDecoder::new(std::fs::File::open(blob)?));
        for entry in archive.entries()? {
            let mut entry = entry?;
            if entry
                .path()?
                .file_name()
                .is_some_and(|name| name.as_encoded_bytes().starts_with(b".wh."))
            {
                continue;
            }
            if !entry.unpack_in(&tree)? {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "archive entry escapes the destination",
                ));
            }
        }
        Ok(())
    })
    .await?
    .map_err(Error::Render)
}

/// Read an optional descriptor annotation.
fn annotation<'a>(layer: &'a OciDescriptor, key: &str) -> Option<&'a str> {
    layer.annotations.as_ref()?.get(key).map(String::as_str)
}

/// Keep digest failures distinct from retryable network failures.
fn transfer_error(error: std::io::Error) -> Error {
    if error
        .get_ref()
        .is_some_and(|source| source.is::<oci_client::errors::DigestError>())
    {
        Error::BlobVerification(error)
    } else {
        Error::Transfer(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::File, io::Write};

    #[tokio::test]
    async fn restore_file_overwrites_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let blob = root.path().join("blob");
        let tree = root.path().join("tree");
        std::fs::write(&blob, b"new").unwrap();
        std::fs::create_dir(&tree).unwrap();
        std::fs::write(tree.join("file"), b"original").unwrap();

        restore_file(&blob, &tree, "file", false).await.unwrap();
        assert_eq!(std::fs::read(tree.join("file")).unwrap(), b"new");
    }

    /// Executable metadata MUST produce a launchable Unix file without changing its bytes.
    #[cfg(unix)]
    #[tokio::test]
    async fn executable_file_restores_launch_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let blob = root.path().join("blob");
        std::fs::write(&blob, b"#!/bin/sh\nexit 0\n").unwrap();
        restore_file(&blob, root.path(), "plugin", true)
            .await
            .unwrap();
        assert_eq!(
            std::fs::metadata(root.path().join("plugin"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            std::fs::read(root.path().join("plugin")).unwrap(),
            b"#!/bin/sh\nexit 0\n"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restore_file_replaces_symlinks_without_writing_to_targets() {
        let root = tempfile::tempdir().unwrap();
        let blob = root.path().join("blob");
        let tree = root.path().join("tree");
        let target = root.path().join("target");
        let missing = root.path().join("missing");
        std::fs::write(&blob, b"new").unwrap();
        std::fs::write(&target, b"original").unwrap();
        std::fs::create_dir(&tree).unwrap();
        std::os::unix::fs::symlink(&target, tree.join("existing")).unwrap();
        std::os::unix::fs::symlink(&missing, tree.join("dangling")).unwrap();

        for name in ["existing", "dangling"] {
            restore_file(&blob, &tree, name, false).await.unwrap();
            assert!(!tree.join(name).symlink_metadata().unwrap().is_symlink());
            assert_eq!(std::fs::read(tree.join(name)).unwrap(), b"new");
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
        assert!(!missing.exists());
    }

    /// Preserve archive paths, replace files, retain unrelated files, and omit whiteouts.
    #[tokio::test]
    async fn restores_directory_artifact_without_whiteout_markers() {
        let root = tempfile::tempdir().unwrap();
        let blob = root.path().join("layer.gz");
        let destination = root.path().join("output");
        let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
            File::create(&blob).unwrap(),
            flate2::Compression::default(),
        ));
        for (path, data) in [
            ("payload/nested/é space", &b"new"[..]),
            ("payload/empty", &b""[..]),
            ("payload/.wh.deleted", &b""[..]),
            ("payload/.wh..wh..opq", &b""[..]),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(data.len() as u64);
            header.set_mtime(1);
            header.set_cksum();
            archive.append_data(&mut header, path, data).unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap();
        std::fs::create_dir_all(destination.join("payload/nested")).unwrap();
        std::fs::write(destination.join("payload/nested/é space"), "old").unwrap();
        std::fs::write(destination.join("unrelated"), "keep").unwrap();
        restore_directory(&blob, &destination, IMAGE_LAYER_GZIP_MEDIA_TYPE)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(destination.join("payload/nested/é space")).unwrap(),
            b"new"
        );
        assert_eq!(
            std::fs::read(destination.join("payload/empty")).unwrap(),
            b""
        );
        assert_eq!(
            std::fs::read(destination.join("unrelated")).unwrap(),
            b"keep"
        );
        assert!(!destination.join("payload/.wh.deleted").exists());
        assert!(!destination.join("payload/.wh..wh..opq").exists());
    }

    /// Corrupt gzip input MUST fail instead of reporting a restored artifact.
    #[tokio::test]
    async fn rejects_corrupt_archive() {
        let root = tempfile::tempdir().unwrap();
        let blob = root.path().join("layer.gz");
        File::create(&blob)
            .unwrap()
            .write_all(b"invalid gzip")
            .unwrap();
        assert!(
            restore_directory(
                &blob,
                &root.path().join("output"),
                IMAGE_LAYER_GZIP_MEDIA_TYPE
            )
            .await
            .is_err()
        );
    }

    /// Archive paths MUST NOT write outside the destination.
    #[tokio::test]
    async fn parent_directory_entries_cannot_escape() {
        let root = tempfile::tempdir().unwrap();
        let blob = root.path().join("layer.gz");
        let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
            File::create(&blob).unwrap(),
            flate2::Compression::default(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(6);
        header.set_mode(0o644);
        header.as_mut_bytes()[..100].fill(0);
        header.as_mut_bytes()[..10].copy_from_slice(b"../outside");
        header.set_cksum();
        archive.append(&header, &b"escape"[..]).unwrap();
        archive.into_inner().unwrap().finish().unwrap();
        let _ = restore_directory(
            &blob,
            &root.path().join("output"),
            IMAGE_LAYER_GZIP_MEDIA_TYPE,
        )
        .await;
        assert!(!root.path().join("outside").exists());
    }
}
