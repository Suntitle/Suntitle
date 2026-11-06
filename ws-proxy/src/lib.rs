//! WebSocket relay and Kubernetes reconciliation controller.
//!
//! Registration opens a host session before dispatch. Acknowledgements advance
//! node operations. Disconnect expires the session before resources are collected.

/// Host, node, and extension reconciliation.
pub mod controller;
mod transport;
