//! Plugin support for locald.
//!
//! This module provides types and utilities for working with locald plugins,
//! including the package manifest format for `.locald-package` archives
//! and the distribution format for `.locald-distribution` archives.

pub mod distribution;
pub mod manifest;

pub use distribution::{
    DistributionCompatibility, DistributionError, DistributionManifest, DistributionMetadata,
    DistributionPlugins, DistributionScaffold, RemotePluginRef, ScaffoldVariable, render_template,
    validate_template_syntax,
};
pub use manifest::{
    CapabilityRequirements, Compatibility, ManifestError, PackageManifest, PackageMetadata,
    PluginSpec,
};
