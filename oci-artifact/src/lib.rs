#![doc = include_str!("../README.md")]
pub mod client;
pub mod constants;
mod download;
mod error;
mod upload;

pub use download::{DownloadRequest, DownloadResult, Downloader};
pub use error::Error;
pub use oci_client::secrets::RegistryAuth;
pub use tokio_util::sync::CancellationToken;
pub use upload::{UploadRequest, UploadResult, Uploader};
