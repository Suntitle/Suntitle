//! Registry client configuration.
use std::path::PathBuf;

use oci_client::client::{Certificate, CertificateEncoding, ClientConfig, ClientProtocol};

use crate::Error;

/// Registry connection settings. HTTPS and certificate verification are defaults.
#[derive(Clone, Debug, Default)]
pub struct TransportOptions {
    /// Use plain HTTP for this registry.
    pub plain_http: bool,
    /// Accept invalid server certificates.
    pub skip_certificate_verification: bool,
    /// Additional trusted certificates in a PEM file.
    pub ca_file: Option<PathBuf>,
}

impl TransportOptions {
    /// Build OCI client configuration from explicit library settings.
    pub fn build_config(&self) -> Result<ClientConfig, Error> {
        let certificates = self
            .ca_file
            .as_ref()
            .map(|p| {
                std::fs::read(p).map(|data| Certificate {
                    encoding: CertificateEncoding::Pem,
                    data,
                })
            })
            .transpose()
            .map_err(Error::Configuration)?;

        Ok(ClientConfig {
            protocol: if self.plain_http {
                ClientProtocol::Http
            } else {
                ClientProtocol::Https
            },
            accept_invalid_certificates: self.skip_certificate_verification,
            extra_root_certificates: certificates.into_iter().collect(),
            read_timeout: Some(std::time::Duration::from_secs(60)),
            connect_timeout: Some(std::time::Duration::from_secs(10)),
            ..Default::default()
        })
    }
}
