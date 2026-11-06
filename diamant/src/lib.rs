//! Host agent execution and extension installation.
//!
//! Artifacts MUST finish downloading and preparation before node processes start.
//! Shutdown requests termination before reaping each remaining child process.

/// Artifact downloads and installation state.
pub mod artifacts;
/// Validated extension entry points and working directories.
pub mod installation;
/// Node process lifecycle and status reporting.
pub mod nodes;
