//! Persist source-only intent. Manifest fetching belongs to the publication controller.
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use suntitle_core::kubernetes::resources::{ExtensionSourceType, SuntitleExtensionSpec};
use crate::{api::errors::ApiError, store::ExtensionCreate};

/// Registration location; all adapter fields come from the source manifest.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionRegistration {
    /// Transport used to read and subsequently publish the extension.
    pub source_type: ExtensionSourceType,
    /// Absolute controller directory, HTTP(S) Git URL, OCI reference, or HTTP payload URL.
    #[schema(min_length = 1)]
    pub source: String,
    /// YAML or JSON manifest contents. Required for source-only OCI registration.
    #[schema(min_length = 1, max_length = 1048576)]
    pub manifest: Option<String>,
}

impl ExtensionRegistration {
    /// Validate request fields and persist source intent without accessing the source.
    pub fn into_create(self) -> Result<ExtensionCreate, ApiError> {
        if self.source.trim().is_empty() {
            return Err(ApiError::problem(
                actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
                "Invalid extension source",
                "Extension path or URL is required.",
            ));
        }
        let invalid = |message| ApiError::problem(
            actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid extension manifest", message,
        );
        if self.source_type == ExtensionSourceType::Oci {
            if self.manifest.as_ref().is_none_or(|manifest| manifest.trim().is_empty()) {
                return Err(invalid("OCI registration requires separate manifest YAML or JSON contents."));
            }
        } else if self.manifest.is_some() {
            return Err(invalid("Separate manifest contents are supported only for OCI sources."));
        }
        if self.manifest.as_ref().is_some_and(|manifest| manifest.len() > 1024 * 1024) {
            return Err(invalid("Extension manifest exceeds 1 MiB."));
        }
        Ok(ExtensionCreate {
            name: None,
            spec: SuntitleExtensionSpec {
                display_name: self.source.chars().take(128).collect(),
                source_type: self.source_type,
                source: self.source,
                manifest: self.manifest,
                refresh_token: None,
                max_attempts: 3,
                adapter: None,
            },
        })
    }
}
