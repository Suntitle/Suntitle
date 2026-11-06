//! Shared resource types, node settings, and host registration protocol.
//!
//! Registration establishes a session before node operations. Each operation is
//! acknowledged before its next lifecycle transition; disconnect ends the session.

/// Kubernetes resources, execution adapters, and bootstrap helpers.
pub mod kubernetes;
/// Serializable node settings and API schema models.
pub mod models;
/// Session messages and operation identities shared by proxy and host agents.
pub mod registration;
