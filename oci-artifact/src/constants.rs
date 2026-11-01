//! ORAS artifact conventions not provided by the OCI client or specification.

/// Descriptor annotation containing the uncompressed directory tar digest.
pub const ANNOTATION_DIGEST: &str = "io.deis.oras.content.digest";

/// Descriptor annotation requesting directory extraction when set to `true`.
pub const ANNOTATION_UNPACK: &str = "io.deis.oras.content.unpack";

/// ORAS default artifact type for content without a caller-defined type.
pub const UNKNOWN_ARTIFACT_MEDIA_TYPE: &str = "application/vnd.unknown.artifact.v1";
