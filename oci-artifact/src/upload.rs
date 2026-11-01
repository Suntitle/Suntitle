//! Package prepared filesystem content using the ORAS file-store convention.
use std::{
    collections::BTreeMap,
    io::{self, Read, Seek, Write},
    path::PathBuf,
};

use flate2::{Compression, write::GzEncoder};
use futures_util::TryStreamExt;
use oci_client::{
    Client, Reference, RegistryOperation,
    annotations::ORG_OPENCONTAINERS_IMAGE_TITLE,
    manifest::{
        IMAGE_LAYER_GZIP_MEDIA_TYPE, IMAGE_LAYER_MEDIA_TYPE, OCI_IMAGE_MEDIA_TYPE, OciDescriptor,
        OciImageManifest, OciManifest,
    },
};
use oci_spec::image::MediaType;
use sha2::{Digest, Sha256};
use tokio_util::io::ReaderStream;

use crate::{
    CancellationToken, Error, RegistryAuth,
    client::TransportOptions,
    constants::{ANNOTATION_DIGEST, ANNOTATION_UNPACK, UNKNOWN_ARTIFACT_MEDIA_TYPE},
};

/// One prepared file or directory and its destination registry reference.
pub struct UploadRequest {
    /// Tag to publish. Digest-only references are unsupported.
    pub reference: String,
    /// Local file or directory. Its basename becomes the artifact title.
    pub source: PathBuf,
    /// Explicit credentials. Defaults to anonymous access.
    pub authentication: RegistryAuth,
    /// Signal cancellation, then await completion.
    pub cancellation: CancellationToken,
}

impl UploadRequest {
    /// Create an anonymous upload of prepared filesystem content.
    pub fn new(reference: impl Into<String>, source: impl Into<PathBuf>) -> Self {
        Self {
            reference: reference.into(),
            source: source.into(),
            authentication: RegistryAuth::Anonymous,
            cancellation: CancellationToken::new(),
        }
    }
}

/// Published artifact identity.
#[derive(Debug)]
pub struct UploadResult {
    /// Immutable digest of the published manifest.
    pub manifest_digest: String,
}

/// Registry client for publishing one file or directory per artifact.
#[derive(Clone)]
pub struct Uploader {
    /// Shared transport and registry authentication state.
    client: Client,
}

impl Uploader {
    /// Construct a registry client with explicit transport settings.
    pub fn new(options: TransportOptions) -> Result<Self, Error> {
        Ok(Self {
            client: Client::try_from(options.build_config()?)?,
        })
    }

    /// Snapshot, package, and publish a prepared file or directory.
    ///
    /// Packaging uses private disk storage and bounded memory. Callers MUST keep
    /// the source stable until this method completes. Cancellation before manifest
    /// publication can leave unreferenced blobs. During publication, the registry
    /// may accept the manifest even if cancellation or a transport error is returned.
    pub async fn upload(&self, request: UploadRequest) -> Result<UploadResult, Error> {
        let reference: Reference = request.reference.parse()?;
        if reference.digest().is_some() {
            return Err(Error::Unsupported("upload requires a tag reference".into()));
        }
        check_cancelled(&request.cancellation)?;
        let token = request.cancellation.clone();
        let packaged = tokio::task::spawn_blocking(move || package(request.source, &token)).await?;
        check_cancelled(&request.cancellation)?;
        let (blob, mut layer) = packaged?;
        let operation = async {
            self.client
                .auth(&reference, &request.authentication, RegistryOperation::Push)
                .await?;
            if layer.size == 0 {
                // The chunked streaming API MUST NOT receive an empty blob.
                layer.digest = format!("sha256:{:x}", Sha256::digest(b""));
                self.client.push_blob(&reference, "", &layer.digest).await?;
            } else {
                let file = tokio::fs::File::from_std(blob.reopen()?);
                let uploaded = self
                    .client
                    .push_blob_stream_chunked(
                        &reference,
                        ReaderStream::new(file).map_err(Into::into),
                    )
                    .await?;
                layer.digest = uploaded.blob_digest;
                layer.size = i64::try_from(uploaded.size)
                    .map_err(|_| Error::Unsupported("source too large".into()))?;
            }
            let uploaded_config = self
                .client
                .push_blob_stream_chunked(
                    &reference,
                    futures_util::stream::once(async {
                        Ok(tokio_util::bytes::Bytes::from_static(b"{}"))
                    }),
                )
                .await?;
            let config = OciDescriptor {
                media_type: MediaType::EmptyJSON.to_string(),
                digest: uploaded_config.blob_digest,
                size: uploaded_config.size as i64,
                ..Default::default()
            };
            let manifest = OciManifest::Image(OciImageManifest {
                media_type: Some(OCI_IMAGE_MEDIA_TYPE.into()),
                artifact_type: Some(UNKNOWN_ARTIFACT_MEDIA_TYPE.into()),
                config,
                layers: vec![layer],
                ..Default::default()
            });
            let location = self.client.push_manifest(&reference, &manifest).await?;
            let digest = location.rsplit('/').next().unwrap().to_owned();
            Ok(UploadResult {
                manifest_digest: digest,
            })
        };
        tokio::select! {
            biased;
            _ = request.cancellation.cancelled() => Err(Error::Cancelled),
            result = operation => result,
        }
    }
}

/// Stop packaging before publishing more local or registry content.
fn check_cancelled(token: &CancellationToken) -> Result<(), Error> {
    if token.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// Reader that bounds cancellation latency during blocking file copies.
struct CheckedReader<R> {
    /// Local content stream being packaged.
    inner: R,
    /// Caller-owned signal checked before every read.
    token: CancellationToken,
}
impl<R: Read> Read for CheckedReader<R> {
    /// Reject further reads after cancellation.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.token.is_cancelled() {
            return Err(io::Error::other("packaging cancelled"));
        }
        self.inner.read(buf)
    }
}

/// Hash accepted tar bytes while forwarding them to the gzip encoder.
struct DigestWriter<W> {
    /// Output receiving the same uncompressed bytes as the digest.
    inner: W,
    /// SHA-256 of bytes successfully accepted by the output.
    digest: Sha256,
}

impl<W: Write> Write for DigestWriter<W> {
    /// Hash only the accepted prefix when the underlying writer writes partially.
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(bytes)?;
        self.digest.update(&bytes[..written]);
        Ok(written)
    }

    /// Flush the encoder without adding bytes to the tar digest.
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Snapshot a prepared source into one blob and its ORAS layer metadata.
fn package(
    source: PathBuf,
    token: &CancellationToken,
) -> Result<(tempfile::NamedTempFile, OciDescriptor), Error> {
    check_cancelled(token)?;
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| Error::Unsupported("source requires a UTF-8 basename".into()))?;
    let metadata = source.symlink_metadata()?;
    let mut blob = tempfile::NamedTempFile::new()?;
    let mut annotations = BTreeMap::from([(ORG_OPENCONTAINERS_IMAGE_TITLE.into(), name.into())]);
    let media_type = if metadata.is_dir() {
        let encoder = GzEncoder::new(blob.as_file_mut(), Compression::default());
        let mut archive = tar::Builder::new(DigestWriter {
            inner: encoder,
            digest: Sha256::new(),
        });
        archive.follow_symlinks(false);
        append_tree(&mut archive, &source, std::path::Path::new(name), token)?;
        let DigestWriter {
            inner: encoder,
            digest,
        } = archive.into_inner()?;
        encoder.finish()?;
        annotations.insert(
            ANNOTATION_DIGEST.into(),
            format!("sha256:{:x}", digest.finalize()),
        );
        annotations.insert(ANNOTATION_UNPACK.into(), "true".into());
        IMAGE_LAYER_GZIP_MEDIA_TYPE
    } else if metadata.is_file() {
        let mut reader = CheckedReader {
            inner: std::fs::File::open(&source)?,
            token: token.clone(),
        };
        io::copy(&mut reader, blob.as_file_mut())?;
        IMAGE_LAYER_MEDIA_TYPE
    } else {
        return Err(Error::Unsupported(
            "source must be a regular file or directory".into(),
        ));
    };
    check_cancelled(token)?;
    blob.as_file_mut().flush()?;
    blob.as_file_mut().rewind()?;
    let descriptor = OciDescriptor {
        media_type: media_type.into(),
        size: i64::try_from(blob.as_file().metadata()?.len())
            .map_err(|_| Error::Unsupported("source too large".into()))?,
        annotations: Some(annotations),
        ..Default::default()
    };
    Ok((blob, descriptor))
}

/// Archive regular files, directories, and symlinks without following links.
fn append_tree<W: Write>(
    archive: &mut tar::Builder<W>,
    source: &std::path::Path,
    name: &std::path::Path,
    token: &CancellationToken,
) -> Result<(), Error> {
    check_cancelled(token)?;
    let metadata = source.symlink_metadata()?;
    let mut header = tar::Header::new_gnu();
    header.set_metadata(&metadata);
    header.set_uid(0);
    header.set_gid(0);
    if metadata.is_dir() {
        archive.append_data(&mut header, name, io::empty())?;
        let mut entries = std::fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            append_tree(archive, &entry.path(), &name.join(entry.file_name()), token)?;
        }
    } else if metadata.is_file() {
        let mut file = CheckedReader {
            inner: std::fs::File::open(source)?,
            token: token.clone(),
        };
        archive.append_data(&mut header, name, &mut file)?;
    } else if metadata.is_symlink() {
        archive.append_link(&mut header, name, std::fs::read_link(source)?)?;
    } else {
        return Err(Error::Unsupported(format!(
            "unsupported source entry: {}",
            source.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_upload_does_not_access_source_or_registry() {
        let uploader = Uploader::new(TransportOptions::default()).unwrap();
        let request = UploadRequest::new("localhost:1/test:cancelled", "/missing-source");
        request.cancellation.cancel();
        assert!(matches!(
            uploader.upload(request).await,
            Err(Error::Cancelled)
        ));
    }

    #[tokio::test]
    async fn missing_source_and_digest_destination_fail_before_network_access() {
        let uploader = Uploader::new(TransportOptions::default()).unwrap();
        let root = tempfile::tempdir().unwrap();
        let request = UploadRequest::new("localhost:1/test:missing", root.path().join("missing"));
        assert!(
            matches!(uploader.upload(request).await, Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotFound)
        );
        let request = UploadRequest::new(
            format!("localhost:1/test@sha256:{}", "a".repeat(64)),
            root.path(),
        );
        assert!(matches!(
            uploader.upload(request).await,
            Err(Error::Unsupported(_))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unsupported_entries_fail_before_network_access() {
        let uploader = Uploader::new(TransportOptions::default()).unwrap();
        let root = tempfile::tempdir().unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(root.path().join("socket")).unwrap();
        let request = UploadRequest::new("localhost:1/test:socket", root.path());
        assert!(matches!(
            uploader.upload(request).await,
            Err(Error::Unsupported(_))
        ));
        let link = root.path().join("link");
        std::os::unix::fs::symlink("socket", &link).unwrap();
        let request = UploadRequest::new("localhost:1/test:link", &link);
        assert!(matches!(
            uploader.upload(request).await,
            Err(Error::Unsupported(_))
        ));
    }
}
