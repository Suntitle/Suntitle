//! Source streams feed registry uploads. Cancellation drops network producers and joins packagers.
use super::{
    PublicationConfig,
    archive::{self, DirectorySource},
    executor::PublishExtension,
};
use futures_util::TryStreamExt;
use oci_artifact::{CancellationToken, PayloadStream, PreparedUpload, Publisher, RegistryAuth};
use std::{io, path::PathBuf, sync::Arc, time::Duration};
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
    pub(super) async fn publish(&self, task: &PublishExtension) -> Result<String, String> {
        let cancellation = self.cancellation.child_token();
        let operation = self.transfer(task, &cancellation);
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
        cancellation: &CancellationToken,
    ) -> Result<String, suntitle_core::kubernetes::Error> {
        let upload = match task.spec.source_type {
            ExtensionSourceType::Http => self.prepare_http(&task.spec.source, cancellation).await?,
            ExtensionSourceType::Local => self.prepare_local(task, cancellation)?,
            ExtensionSourceType::Git => self.prepare_git(task, cancellation).await?,
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
        let url = reqwest::Url::parse(source)?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("HTTP source requires an HTTP(S) URL without embedded credentials".into());
        }
        let filename = url
            .path_segments()
            .and_then(|mut parts| parts.next_back())
            .filter(|name| {
                !name.is_empty() && *name != "adapter.yaml" && *name != "." && *name != ".."
            })
            .unwrap_or("payload")
            .to_owned();
        let authority = &url[url::Position::BeforeHost..url::Position::AfterPort];
        let mut request = self.http.get(url.clone());
        if let Some(RegistryAuth::Basic(username, password)) =
            self.config.source_authentication.get(authority)
        {
            request = request.basic_auth(username, Some(password));
        }
        let response = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err("publication cancelled".into()),
            result = request.send() => result?.error_for_status()?,
        };
        let content: PayloadStream = Box::pin(response.bytes_stream().map_err(io::Error::other));
        Ok(PreparedUpload::file(filename, content).executable())
    }

    /// Package a HUB directory with the desired adapter specification.
    fn prepare_local(
        &self,
        task: &PublishExtension,
        cancellation: &CancellationToken,
    ) -> Result<PreparedUpload, suntitle_core::kubernetes::Error> {
        let source = PathBuf::from(&task.spec.source);
        if !source.is_absolute() || !source.is_dir() {
            return Err("local source MUST be an absolute HUB directory".into());
        }
        Ok(archive::prepare(
            DirectorySource::Local(source),
            serde_yaml::to_string(&task.spec.adapter)?.into_bytes(),
            cancellation.clone(),
        ))
    }

    /// Package the pinned Git snapshot with the desired adapter specification.
    async fn prepare_git(
        &self,
        task: &PublishExtension,
        cancellation: &CancellationToken,
    ) -> Result<PreparedUpload, suntitle_core::kubernetes::Error> {
        let authority = url::Url::parse(&task.spec.source)
            .ok()
            .map(|url| url[url::Position::BeforeHost..url::Position::AfterPort].to_owned());
        let authentication = authority
            .as_ref()
            .and_then(|host| self.config.source_authentication.get(host));
        let snapshot =
            super::git::Snapshot::fetch(&task.spec.source, authentication, cancellation).await?;
        Ok(archive::prepare(
            DirectorySource::Git(snapshot),
            serde_yaml::to_string(&task.spec.adapter)?.into_bytes(),
            cancellation.clone(),
        ))
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
