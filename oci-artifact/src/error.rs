//! Typed download errors.
use std::io;

/// Registry, verification, extraction, and local output failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The registry reference could not be parsed.
    #[error("invalid registry reference: {0}")]
    Reference(#[from] oci_client::ParseError),
    /// Registry access or client construction failed.
    #[error("registry failure: {0}")]
    Registry(#[from] oci_client::errors::OciDistributionError),
    /// The downloaded stream did not match its descriptor digest.
    #[error("blob digest verification failed: {0}")]
    BlobVerification(#[source] io::Error),
    /// Reading the registry response failed.
    #[error("blob transfer failure: {0}")]
    Transfer(#[source] io::Error),
    /// Reading or writing local files failed.
    #[error("local I/O failure: {0}")]
    Io(#[from] io::Error),
    /// The configured CA file could not be read.
    #[error("client configuration failure: {0}")]
    Configuration(#[source] io::Error),
    /// The manifest does not describe a supported file or directory artifact.
    #[error("unsupported artifact: {0}")]
    Unsupported(String),
    /// The blob size does not match its descriptor.
    #[error("blob verification failed: {0}")]
    Verification(String),
    /// Decompression or extraction failed.
    #[error("rendering failed: {0}")]
    Render(#[from] ocirender::Error),
    /// The caller cancelled the download.
    #[error("download cancelled")]
    Cancelled,
}
