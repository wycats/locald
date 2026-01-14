//! Plugin support for locald.
//!
//! This module provides types and utilities for working with locald plugins,
//! including the package manifest format for `.locald-package` archives.

pub mod manifest;

pub use manifest::{
    CapabilityRequirements, Compatibility, ManifestError, PackageManifest, PackageMetadata,
    PluginSpec,
};
