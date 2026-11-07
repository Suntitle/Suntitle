//! HTTP control API and Kubernetes resource storage.

/// HTTP routes, authentication, errors, and OpenAPI schemas.
pub mod api;
/// Kubernetes persistence used by the control API.
pub mod store;

/// Source manifest discovery for extension registration.
pub mod extension_source;
