//! Shared source validation used before manifest and payload transfers.
use std::{io, path::{Path, PathBuf}};
use suntitle_core::kubernetes::Error;

/// Require HTTP(S) without URL credentials before selecting authority-scoped authentication.
pub(super) fn http_url(source: &str) -> Result<url::Url, Error> {
    let url = url::Url::parse(source).map_err(|_| invalid("Source requires an HTTP(S) URL."))?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return Err(invalid("Source requires HTTP(S) without embedded credentials; SSH is unsupported."));
    }
    Ok(url)
}

/// Require a worker-accessible absolute directory for discovery and packaging.
pub(super) fn local_directory(source: &Path) -> Result<PathBuf, Error> {
    let path = source.to_path_buf();
    if !path.is_absolute() || !path.is_dir() {
        return Err(invalid("Local source MUST be an absolute directory accessible to the controller."));
    }
    Ok(path)
}

/// Preserve validation failures as safe publication status messages.
pub(super) fn invalid(message: &str) -> Error {
    io::Error::new(io::ErrorKind::InvalidInput, message).into()
}
