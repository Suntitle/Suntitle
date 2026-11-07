//! Incremental publication. Blobs finish before manifest publication and digest verification.
//! Failed or cancelled transfers leave no published manifest; registries may retain orphan blobs.
use crate::{
    CancellationToken, Error, RegistryAuth,
    client::TransportOptions,
    constants::{
        IMAGE_LAYER_GZIP_MEDIA_TYPE, IMAGE_LAYER_MEDIA_TYPE, MediaType, OCI_IMAGE_MEDIA_TYPE,
        ORG_OPENCONTAINERS_IMAGE_TITLE,
    },
};
use futures_util::{Stream, StreamExt, TryStreamExt};
use oci_client::{
    Client, Reference, RegistryOperation,
    manifest::{OciDescriptor, OciImageManifest, OciManifest},
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io, pin::Pin};
use tokio_util::bytes::Bytes;

/// Bounded asynchronous payload source. Producers MUST apply backpressure.
pub type PayloadStream = Pin<Box<dyn Stream<Item = Result<Bytes, io::Error>> + Send>>;

/// One layer and its optional archive producer, owned until publication cleanup finishes.
pub struct PreparedUpload {
    /// Media type understood by the file or directory restoration path.
    media_type: String,
    /// ORAS filename and restoration annotations.
    annotations: BTreeMap<String, String>,
    /// Payload consumed with registry backpressure.
    content: PayloadStream,
    /// Archive task returning metadata computed while producing bytes.
    producer: Option<tokio::task::JoinHandle<io::Result<BTreeMap<String, String>>>>,
    /// Stops archive work on publication failure or cancellation.
    cancellation: CancellationToken,
}

/// Registry publication transport shared across background jobs.
#[derive(Clone)]
pub struct Publisher {
    /// Pooled HTTP transport and cached authentication.
    client: Client,
}
impl Publisher {
    /// Build a pooled registry client with explicit TLS and timeout settings.
    pub fn new(options: TransportOptions) -> Result<Self, Error> {
        Ok(Self {
            client: Client::try_from(options.build_config()?)?,
        })
    }

    /// Reject destination prefixes that cannot form immutable per-extension tags.
    pub fn validate_repository(repository: &str) -> Result<(), Error> {
        let reference: Reference =
            format!("{}/validation:attempt", repository.trim_end_matches('/')).parse()?;
        if reference.tag() != Some("attempt") || reference.digest().is_some() {
            return Err(Error::Unsupported(
                "invalid registry repository prefix".into(),
            ));
        }
        Ok(())
    }

    /// Upload one layer, finish its producer, then publish and verify the manifest.
    pub async fn publish(
        &self,
        target: &str,
        auth: &RegistryAuth,
        mut upload: PreparedUpload,
        cancellation: &CancellationToken,
    ) -> Result<String, Error> {
        let result = {
            let operation = async {
                let target: Reference = target.parse()?;
                self.client
                    .auth(&target, auth, RegistryOperation::Push)
                    .await?;
                let content =
                    std::mem::replace(&mut upload.content, Box::pin(futures_util::stream::empty()));
                let mut content = content.filter(|item| {
                    std::future::ready(!matches!(item, Ok(bytes) if bytes.is_empty()))
                });
                let first = content.next().await;
                let descriptor = if first.is_none() {
                    let digest = format!("sha256:{}", hex::encode(Sha256::digest(b"")));
                    self.client.push_blob(&target, "", &digest).await?;
                    OciDescriptor {
                        digest,
                        size: 0,
                        ..Default::default()
                    }
                } else {
                    let uploaded = self
                        .client
                        .push_blob_stream_chunked(
                            &target,
                            futures_util::stream::iter(first)
                                .chain(content)
                                .map_err(|e| {
                                    oci_client::errors::OciDistributionError::GenericError(Some(
                                        e.to_string(),
                                    ))
                                }),
                        )
                        .await?;
                    OciDescriptor {
                        digest: uploaded.blob_digest,
                        size: uploaded
                            .size
                            .try_into()
                            .map_err(|_| Error::Unsupported("layer too large".into()))?,
                        ..Default::default()
                    }
                };
                upload.finish().await?;
                let layer = OciDescriptor {
                    media_type: upload.media_type.clone(),
                    annotations: Some(upload.annotations.clone()),
                    ..descriptor
                };
                let digest = format!("sha256:{}", hex::encode(Sha256::digest(b"{}")));
                self.client.push_blob(&target, "{}", &digest).await?;
                let manifest = OciManifest::Image(OciImageManifest {
                    media_type: Some(OCI_IMAGE_MEDIA_TYPE.into()),
                    artifact_type: Some(crate::constants::UNKNOWN_ARTIFACT_MEDIA_TYPE.into()),
                    config: OciDescriptor {
                        media_type: MediaType::EmptyJSON.to_string(),
                        digest,
                        size: 2,
                        ..Default::default()
                    },
                    layers: vec![layer],
                    ..Default::default()
                });
                let location = self.client.push_manifest(&target, &manifest).await?;
                let digest = location
                    .rsplit('/')
                    .next()
                    .ok_or_else(|| Error::Unsupported("registry omitted manifest digest".into()))?;
                let reference = format!("{}/{}@{}", target.registry(), target.repository(), digest);
                self.verify(&reference, auth).await?;
                Ok(reference)
            };
            tokio::select! { biased; _ = cancellation.cancelled() => Err(Error::Cancelled), result = operation => result }
        };
        drop(std::mem::replace(
            &mut upload.content,
            Box::pin(futures_util::stream::empty()),
        ));
        upload.cancellation.cancel();
        let cleanup = upload.finish().await;
        match result {
            Ok(reference) => {
                cleanup?;
                Ok(reference)
            }
            Err(error) => Err(error),
        }
    }

    /// Confirm that the canonical digest reference resolves through the OCI client.
    pub async fn verify(&self, reference: &str, auth: &RegistryAuth) -> Result<(), Error> {
        let reference: Reference = reference.parse()?;
        self.client.pull_manifest(&reference, auth).await?;
        Ok(())
    }

    /// Stream an upstream artifact unchanged without application-level layout checks.
    pub async fn republish(
        &self,
        source: &str,
        target: &str,
        source_auth: &RegistryAuth,
        target_auth: &RegistryAuth,
        cancellation: &CancellationToken,
    ) -> Result<String, Error> {
        let source: Reference = source.parse()?;
        let target: Reference = target.parse()?;
        let operation = async {
            let (raw, digest) = self
                .client
                .pull_manifest_raw(&source, source_auth, &[OCI_IMAGE_MEDIA_TYPE])
                .await?;
            let manifest: OciImageManifest = serde_json::from_slice(&raw)
                .map_err(|_| Error::Unsupported("expected an OCI artifact manifest".into()))?;
            let pinned: Reference =
                format!("{}/{}@{}", source.registry(), source.repository(), digest).parse()?;
            self.client
                .auth(&target, target_auth, RegistryOperation::Push)
                .await?;
            for descriptor in std::iter::once(&manifest.config).chain(&manifest.layers) {
                let stream = self.client.pull_blob_stream(&pinned, descriptor).await?;
                if descriptor.size == 0 {
                    // Consuming EOF is required for client-owned digest verification.
                    let mut stream = stream.stream;
                    while let Some(chunk) = stream.try_next().await.map_err(Error::Transfer)? {
                        if !chunk.is_empty() {
                            return Err(Error::Verification("nonempty zero-sized blob".into()));
                        }
                    }
                    self.client
                        .push_blob(&target, "", &descriptor.digest)
                        .await?;
                } else {
                    self.client
                        .push_blob_stream(
                            &target,
                            stream.stream.map_err(|e| {
                                oci_client::errors::OciDistributionError::GenericError(Some(
                                    e.to_string(),
                                ))
                            }),
                            &descriptor.digest,
                            Some(descriptor.size as u64),
                        )
                        .await?;
                }
            }
            self.client
                .push_manifest_raw(&target, raw, OCI_IMAGE_MEDIA_TYPE.parse().unwrap())
                .await?;
            let reference = format!("{}/{}@{}", target.registry(), target.repository(), digest);
            self.verify(&reference, target_auth).await?;
            Ok(reference)
        };
        tokio::select! { biased; _ = cancellation.cancelled() => Err(Error::Cancelled), result = operation => result }
    }
}

impl PreparedUpload {
    /// Prepare one raw file, including empty files and unknown response sizes.
    pub fn file(name: String, content: PayloadStream) -> Self {
        Self {
            media_type: IMAGE_LAYER_MEDIA_TYPE.into(),
            annotations: BTreeMap::from([(ORG_OPENCONTAINERS_IMAGE_TITLE.into(), name)]),
            content,
            producer: None,
            cancellation: CancellationToken::new(),
        }
    }

    /// Request executable file permissions on the consuming host.
    pub fn executable(mut self) -> Self {
        self.annotations.insert(
            crate::constants::ANNOTATION_EXECUTABLE.into(),
            "true".into(),
        );
        self
    }

    /// Own a streamed directory archive and join its producer before publishing a manifest.
    pub fn directory(
        content: PayloadStream,
        producer: tokio::task::JoinHandle<io::Result<BTreeMap<String, String>>>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            media_type: IMAGE_LAYER_GZIP_MEDIA_TYPE.into(),
            annotations: BTreeMap::from([
                (ORG_OPENCONTAINERS_IMAGE_TITLE.into(), "extension".into()),
                (crate::constants::ANNOTATION_UNPACK.into(), "true".into()),
            ]),
            content,
            producer: Some(producer),
            cancellation,
        }
    }

    /// Join archive production and collect metadata exactly once.
    async fn finish(&mut self) -> Result<(), Error> {
        if let Some(producer) = self.producer.as_mut() {
            let result = producer.await;
            self.producer = None;
            self.annotations.extend(result??);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::io::AsyncWriteExt;
    use tokio_util::io::ReaderStream;

    /// Publication MUST join a blocked producer before returning failure or cancellation.
    #[tokio::test]
    async fn failed_or_cancelled_publication_stops_and_joins_archive_production() {
        for cancelled in [false, true] {
            let (read, mut write) = tokio::io::duplex(8);
            let stopped = Arc::new(AtomicBool::new(false));
            let finished = stopped.clone();
            let producer = tokio::spawn(async move {
                let result = write.write_all(&[0; 1024]).await;
                finished.store(true, Ordering::SeqCst);
                result?;
                Ok(BTreeMap::new())
            });
            let upload = PreparedUpload::directory(
                Box::pin(ReaderStream::new(read)),
                producer,
                CancellationToken::new(),
            );
            let cancellation = CancellationToken::new();
            if cancelled {
                cancellation.cancel();
            }
            let publisher = Publisher::new(TransportOptions::default()).unwrap();
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                publisher.publish(
                    "invalid reference",
                    &RegistryAuth::Anonymous,
                    upload,
                    &cancellation,
                ),
            )
            .await
            .unwrap();
            assert!(result.is_err());
            assert!(stopped.load(Ordering::SeqCst));
        }
    }
}
