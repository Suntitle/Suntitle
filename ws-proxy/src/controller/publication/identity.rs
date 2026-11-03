//! A stable Apalis task identity covers one desired resource revision and destination.
//! Retries reuse that identity and remain entirely owned by SQLite storage.
use super::PublicationConfig;
use crate::kubernetes::{Error, resources::SuntitleExtension};
use apalis::prelude::TaskId;
use kube::ResourceExt;
use sha2::{Digest, Sha256};

/// Hash immutable resource inputs into the ULID representation required by Apalis SQLite.
pub(super) fn task_id(
    extension: &SuntitleExtension,
    config: &PublicationConfig,
) -> Result<TaskId, Error> {
    let input = serde_json::to_vec(&(
        extension.uid(),
        extension.metadata.generation,
        &extension.spec,
        &config.repository,
        config.transport.plain_http,
        config.transport.skip_certificate_verification,
        &config.transport.ca_file,
    ))?;
    let digest = Sha256::digest(input);
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    Ok(TaskId::from_ulid(u128::from_be_bytes(bytes).into()))
}
