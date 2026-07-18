//! Compatibility names for the predecessor path-oriented registry surface.
//!
//! The daemon now stores authoritative identity relationships in
//! [`ProjectCatalog`]. Existing IPC and CLI names remain available as a
//! path-based projection during the beta compatibility period.

use crate::catalog::ProjectCatalog;
pub use crate::catalog::ProjectEntry;

/// Compatibility alias for callers that still refer to the project registry.
pub type Registry = ProjectCatalog;
