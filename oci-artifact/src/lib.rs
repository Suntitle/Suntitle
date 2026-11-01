#![doc = include_str!("../README.md")]
pub mod client;
mod download;
mod error;

pub use download::{DownloadRequest, DownloadResult, Downloader};
pub use error::Error;
pub use oci_client::secrets::RegistryAuth;
pub use tokio_util::sync::CancellationToken;
