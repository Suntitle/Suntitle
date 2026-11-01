//! Execution identity combines desired resource inputs with one controller retry attempt.
use super::PublicationConfig;
use crate::kubernetes::{Error, resources::SuntitleExtension};
use kube::ResourceExt;
use sha2::{Digest, Sha256};

/// One resource revision, destination, and attempt. Its serialized form is stored in status.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct ExecutionId {
    /// Hash of immutable resource identity, desired specification, and destination settings.
    desired: String,
    /// Zero-based controller retry number.
    attempt: u32,
}
impl ExecutionId {
    /// Identify desired preparation without reading source contents.
    pub fn for_extension(
        extension: &SuntitleExtension,
        config: &PublicationConfig,
    ) -> Result<Self, Error> {
        let input = serde_json::to_vec(&(
            extension.uid(),
            extension.metadata.generation,
            &extension.spec,
            &config.repository,
            config.transport.plain_http,
            config.transport.skip_certificate_verification,
            &config.transport.ca_file,
        ))?;
        Ok(Self {
            desired: format!("{:x}", Sha256::digest(input)),
            attempt: 0,
        })
    }
    /// Decode persisted identity, rejecting malformed hashes and retry numbers.
    pub fn parse(value: &str) -> Option<Self> {
        let (desired, attempt) = value.rsplit_once('-')?;
        if desired.len() != 64 || !desired.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        Some(Self {
            desired: desired.into(),
            attempt: attempt.parse().ok()?,
        })
    }
    /// Serialize the stable status and executor identity.
    pub fn encode(&self) -> String {
        format!("{}-{}", self.desired, self.attempt)
    }
    /// Check desired inputs independently of retry number.
    pub fn matches(&self, desired: &Self) -> bool {
        self.desired == desired.desired
    }
    /// Return the zero-based controller retry number.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }
    /// Identify the next attempt without changing desired inputs.
    pub fn next_attempt(&self) -> Self {
        Self {
            desired: self.desired.clone(),
            attempt: self.attempt + 1,
        }
    }
    /// Construct a tag unique to this resource revision and attempt.
    pub fn destination(&self, repository: &str, name: &str) -> String {
        format!(
            "{}/{}:attempt-{}",
            repository.trim_end_matches('/'),
            name,
            self.encode()
        )
    }
}
