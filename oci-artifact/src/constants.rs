//! Shared OCI metadata and ORAS/Suntitle conventions.

/// Standard filename annotation supplied by the OCI client.
pub use oci_client::annotations::ORG_OPENCONTAINERS_IMAGE_TITLE;
/// Standard layer and manifest media types supplied by the OCI client.
pub use oci_client::manifest::{
    IMAGE_LAYER_GZIP_MEDIA_TYPE, IMAGE_LAYER_MEDIA_TYPE, OCI_IMAGE_MEDIA_TYPE,
};
/// OCI specification media types, including the empty JSON configuration type.
pub use oci_spec::image::MediaType;

/// Descriptor annotation containing the uncompressed directory tar digest.
pub const ANNOTATION_DIGEST: &str = "io.deis.oras.content.digest";

/// Descriptor annotation requesting directory extraction when set to `true`.
pub const ANNOTATION_UNPACK: &str = "io.deis.oras.content.unpack";

/// ORAS default artifact type for content without a caller-defined type.
pub const UNKNOWN_ARTIFACT_MEDIA_TYPE: &str = "application/vnd.unknown.artifact.v1";

/// Named file MUST be restored with executable permissions on Unix hosts.
pub const ANNOTATION_EXECUTABLE: &str = "io.suntitle.executable";
