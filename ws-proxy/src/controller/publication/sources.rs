//! Source streams feed registry uploads. Cancellation drops network producers and joins packagers.
use super::{
    PublicationConfig,
    archive::{self, DirectorySource},
    executor::{PublishExtension, PublicationResult},
};
use futures_util::TryStreamExt;
use oci_artifact::{CancellationToken, PayloadStream, PreparedUpload, Publisher, RegistryAuth};
use std::{io, sync::Arc, time::Duration};
use suntitle_core::kubernetes::resources::ExtensionSourceType;

/// Shared source clients and destination credentials. Secrets never enter task identity.
#[derive(Clone)]
pub(in crate::controller) struct Sources {
    /// Pooled source HTTP client.
    http: reqwest::Client,
    /// Pooled OCI transport for pull and push.
    registry: Publisher,
    /// Explicit destination and transport settings.
    config: Arc<PublicationConfig>,
    /// Controller shutdown cancellation signal.
    cancellation: CancellationToken,
}
impl Sources {
    /// Validate transport configuration before starting controllers.
    pub fn new(
        config: PublicationConfig,
        cancellation: CancellationToken,
    ) -> Result<Self, suntitle_core::kubernetes::Error> {
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .read_timeout(Duration::from_secs(60))
                .build()?,
            registry: Publisher::new(config.transport.clone())?,
            config: Arc::new(config),
            cancellation,
        })
    }
    /// Prepare one immutable source attempt and verify its published artifact.
    pub(super) async fn publish(&self, task: &PublishExtension) -> Result<PublicationResult, String> {
        let cancellation = self.cancellation.child_token();
        let operation = async {
            let mut resolved = task.clone();
            let snapshot = if resolved.spec.adapter.is_none()
                || (resolved.spec.source_type == ExtensionSourceType::Oci && resolved.spec.manifest.is_some())
            {
                let (spec, snapshot) = super::manifest::resolve(&task.spec, self, &cancellation).await?;
                resolved.spec = spec;
                snapshot
            } else {
                None
            };
            resolved.spec.manifest = task.spec.manifest.clone();
            resolved.spec.refresh_token = task.spec.refresh_token.clone();
            resolved.spec.max_attempts = task.spec.max_attempts;
            let reference = self.transfer(&resolved, snapshot, &cancellation).await?;
            Ok::<_, suntitle_core::kubernetes::Error>(PublicationResult { reference, spec: resolved.spec })
        };
        tokio::pin!(operation);
        tokio::select! {
            result = &mut operation => result.map_err(|error| {
                if let Some(error) = error.downcast_ref::<io::Error>()
                    && error.kind() == io::ErrorKind::InvalidInput
                { return error.to_string(); }
                "Extension publication failed. Check source access, registry credentials, and connectivity.".into()
            }),
            _ = tokio::time::sleep(self.config.task_timeout) => {
                cancellation.cancel();
                let _ = operation.await;
                Err("Extension publication exceeded its configured deadline.".into())
            }
        }
    }
    /// Select source preparation, then publish its single layer.
    async fn transfer(
        &self,
        task: &PublishExtension,
        snapshot: Option<super::git::Snapshot>,
        cancellation: &CancellationToken,
    ) -> Result<String, suntitle_core::kubernetes::Error> {
        let upload = match task.spec.source_type {
            ExtensionSourceType::Http => self.prepare_http(&task.spec.source, cancellation).await?,
            ExtensionSourceType::Local => self.prepare_local(task, cancellation)?,
            ExtensionSourceType::Git => match snapshot {
                Some(snapshot) => self.package(task, DirectorySource::Git(snapshot), cancellation)?,
                None => self.prepare_git(task, cancellation).await?,
            },
            ExtensionSourceType::Oci => return self.republish_oci(task, cancellation).await,
        };
        Ok(self
            .registry
            .publish(
                &task.destination,
                &self.config.authentication,
                upload,
                cancellation,
            )
            .await?)
    }

    /// Stream the HTTP response as a single executable file.
    async fn prepare_http(
        &self,
        source: &str,
        cancellation: &CancellationToken,
    ) -> Result<PreparedUpload, suntitle_core::kubernetes::Error> {
        let url = super::transport::http_url(source)?;
        let filename = url
            .path_segments()
            .and_then(|mut parts| parts.next_back())
            .filter(|name| {
                !name.is_empty() && *name != "adapter.yaml" && *name != "." && *name != ".."
            })
            .unwrap_or("payload")
            .to_owned();
        let response = self.http_get(url, None, cancellation).await?.error_for_status()?;
        let content: PayloadStream = Box::pin(response.bytes_stream().map_err(io::Error::other));
        Ok(PreparedUpload::file(filename, content).executable())
    }

    /// Package a HUB directory with the desired adapter specification.
    fn prepare_local(
        &self,
        task: &PublishExtension,
        cancellation: &CancellationToken,
    ) -> Result<PreparedUpload, suntitle_core::kubernetes::Error> {
        self.package(task, DirectorySource::Local(super::transport::local_directory(std::path::Path::new(&task.spec.source))?), cancellation)
    }

    /// Fetch one pinned Git snapshot using the configured source credentials.
    pub(super) async fn fetch_git(
        &self,
        source: &str,
        cancellation: &CancellationToken,
    ) -> Result<super::git::Snapshot, suntitle_core::kubernetes::Error> {
        let url = super::transport::http_url(source)?;
        let authority = &url[url::Position::BeforeHost..url::Position::AfterPort];
        super::git::Snapshot::fetch(source, self.config.source_authentication.get(authority), cancellation).await
    }

    /// Fetch payload bytes or sibling manifests through the same authenticated HTTP client.
    pub(super) async fn http_get(
        &self,
        url: url::Url,
        timeout: Option<Duration>,
        cancellation: &CancellationToken,
    ) -> Result<reqwest::Response, suntitle_core::kubernetes::Error> {
        let authority = &url[url::Position::BeforeHost..url::Position::AfterPort];
        let mut request = self.http.get(url.clone());
        if let Some(RegistryAuth::Basic(username, password)) = self.config.source_authentication.get(authority) {
            request = request.basic_auth(username, Some(password));
        }
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err("publication cancelled".into()),
            result = request.send() => Ok(result?),
        }
    }

    /// Package a directory or pinned Git tree with one resolved adapter serialization.
    fn package(
        &self,
        task: &PublishExtension,
        source: DirectorySource,
        cancellation: &CancellationToken,
    ) -> Result<PreparedUpload, suntitle_core::kubernetes::Error> {
        let adapter = task.spec.adapter.as_ref().ok_or("extension manifest is unresolved")?;
        Ok(archive::prepare(source, serde_yaml::to_string(adapter)?.into_bytes(), cancellation.clone()))
    }

    /// Package an explicit Git configuration using the same snapshot transport as discovery.
    async fn prepare_git(
        &self,
        task: &PublishExtension,
        cancellation: &CancellationToken,
    ) -> Result<PreparedUpload, suntitle_core::kubernetes::Error> {
        let snapshot = self.fetch_git(&task.spec.source, cancellation).await?;
        self.package(task, DirectorySource::Git(snapshot), cancellation)
    }

    /// Copy upstream OCI bytes directly without source preparation.
    async fn republish_oci(
        &self,
        task: &PublishExtension,
        cancellation: &CancellationToken,
    ) -> Result<String, suntitle_core::kubernetes::Error> {
        let authority = task.spec.source.split('/').next().unwrap_or_default();
        let authentication = self
            .config
            .source_authentication
            .get(authority)
            .unwrap_or(&RegistryAuth::Anonymous);
        Ok(self
            .registry
            .republish(
                &task.spec.source,
                &task.destination,
                authentication,
                &self.config.authentication,
                cancellation,
            )
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use warp::Filter;

    /// Manifest and payload requests MUST share authentication and obey cancellation.
    #[tokio::test]
    async fn manifest_and_payload_use_shared_authenticated_http_transport() {
        let (send, mut received) = tokio::sync::mpsc::unbounded_channel();
        let route = warp::path::full()
            .and(warp::header::optional::<String>("authorization"))
            .map(move |path: warp::path::FullPath, authorization: Option<String>| {
                send.send(path.as_str().to_owned()).unwrap();
                let body = if path.as_str().ends_with("manifest.yaml") {
                    "displayName: HTTP fixture\nadapter:\n  execution:\n    command: ./hello".to_string()
                } else {
                    "binary payload".to_string()
                };
                let status = if authorization.as_deref() == Some("Basic dXNlcjpwYXNz") {
                    warp::http::StatusCode::OK
                } else {
                    warp::http::StatusCode::UNAUTHORIZED
                };
                warp::reply::with_status(body, status)
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(warp::serve(route).incoming(listener).run());
        let mut config = super::super::tests::config();
        config.source_authentication.insert(address.to_string(), RegistryAuth::Basic("user".into(), "pass".into()));
        let token = CancellationToken::new();
        let sources = Sources::new(config, token.clone()).unwrap();
        let source = format!("http://{address}/payload/hello");
        let spec = serde_json::from_value(serde_json::json!({"displayName":"Pending", "sourceType":"http", "source":source})).unwrap();
        let (resolved, snapshot) = super::super::manifest::resolve(&spec, &sources, &token).await.unwrap();
        assert_eq!(resolved.display_name, "HTTP fixture");
        assert!(snapshot.is_none());
        sources.prepare_http(&source, &token).await.unwrap();
        assert_eq!(received.recv().await.unwrap(), "/payload/manifest.yaml");
        assert_eq!(received.recv().await.unwrap(), "/payload/hello");
        token.cancel();
        assert!(sources.http_get(super::super::transport::http_url(&source).unwrap(), None, &token).await.is_err());
        assert!(received.try_recv().is_err());
        server.abort();
    }
}
