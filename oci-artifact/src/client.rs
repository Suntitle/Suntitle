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
    /// Restrict development HTTP to one registry authority; upstreams MUST retain HTTPS.
    pub fn build_config_for_registry(&self, registry: &str) -> Result<ClientConfig, Error> {
        let mut config = self.build_config()?;
        if self.plain_http {
            config.protocol = ClientProtocol::HttpsExcept(vec![registry.to_owned()]);
        }
        Ok(config)
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A local HTTP destination MUST NOT downgrade external source registries.
    #[test]
    fn development_http_is_scoped_to_destination() {
        let options = TransportOptions { plain_http: true, ..Default::default() };
        let config = options.build_config_for_registry("registry.localhost:5000").unwrap();
        assert!(matches!(config.protocol, ClientProtocol::HttpsExcept(ref authorities)
            if authorities == &["registry.localhost:5000"]));
        let config = TransportOptions::default().build_config_for_registry("ghcr.io").unwrap();
        assert!(matches!(config.protocol, ClientProtocol::Https));
    }
}
