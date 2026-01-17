//! Package manifest schema for `.locald-package` files.
//!
//! This module defines the manifest format specified in RFC 0129, section 3.13.2.
//! The manifest is a TOML file (`manifest.toml`) that describes a locald plugin package.
//!
//! # Example
//!
//! ```toml
//! [package]
//! name = "redis-plugin"
//! version = "1.0.0"
//! description = "Redis service support"
//!
//! [plugin]
//! component = "plugin.wasm"
//! service_kinds = ["redis"]
//!
//! [compatibility]
//! ir_version = 1
//!
//! [capabilities]
//! required = ["oci_pull"]
//! ```

use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

/// Maximum length for package names.
const MAX_NAME_LENGTH: usize = 64;

/// Errors that can occur when parsing or validating a package manifest.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// Failed to parse the manifest TOML.
    #[error("failed to parse manifest: {0}")]
    Parse(#[from] toml::de::Error),

    /// Package name is invalid.
    #[error("invalid package name '{name}': {reason}")]
    InvalidName {
        /// The invalid name.
        name: String,
        /// Why the name is invalid.
        reason: String,
    },

    /// Package version is not valid semver.
    #[error("invalid package version '{version}': {reason}")]
    InvalidVersion {
        /// The invalid version string.
        version: String,
        /// Why the version is invalid.
        reason: String,
    },

    /// Plugin component path is invalid.
    #[error("invalid plugin component path '{path}': {reason}")]
    InvalidComponentPath {
        /// The invalid path.
        path: String,
        /// Why the path is invalid.
        reason: String,
    },

    /// IR version is invalid.
    #[error("invalid IR version {version}: must be positive")]
    InvalidIrVersion {
        /// The invalid version.
        version: i32,
    },

    /// Required field is missing.
    #[error("missing required field: {field}")]
    MissingField {
        /// The missing field name.
        field: String,
    },
}

/// Root structure for a package manifest.
///
/// Corresponds to the `manifest.toml` file in a `.locald-package` archive.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageManifest {
    /// Package metadata (`[package]` section).
    pub package: PackageMetadata,

    /// Plugin specification (`[plugin]` section).
    pub plugin: PluginSpec,

    /// Compatibility requirements (`[compatibility]` section).
    #[serde(default)]
    pub compatibility: Compatibility,

    /// Capability requirements (`[capabilities]` section).
    #[serde(default)]
    pub capabilities: CapabilityRequirements,
}

impl PackageManifest {
    /// Parse a manifest from TOML string content.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing fails or validation fails.
    pub fn parse(content: &str) -> Result<Self, ManifestError> {
        let manifest: Self = toml::from_str(content)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate all fields of the manifest.
    ///
    /// # Errors
    ///
    /// Returns the first validation error encountered.
    pub fn validate(&self) -> Result<(), ManifestError> {
        self.validate_name()?;
        self.validate_version()?;
        self.validate_component_path()?;
        self.validate_ir_version()?;
        Ok(())
    }

    /// Validate the package name.
    fn validate_name(&self) -> Result<(), ManifestError> {
        let name = &self.package.name;

        if name.is_empty() {
            return Err(ManifestError::InvalidName {
                name: name.clone(),
                reason: "name cannot be empty".to_string(),
            });
        }

        if name.len() > MAX_NAME_LENGTH {
            return Err(ManifestError::InvalidName {
                name: name.clone(),
                reason: format!("name exceeds maximum length of {MAX_NAME_LENGTH} characters"),
            });
        }

        // Must match: ^[a-z][a-z0-9-]*$
        let mut chars = name.chars();

        // First character must be lowercase letter
        match chars.next() {
            Some(c) if c.is_ascii_lowercase() => {}
            Some(c) => {
                return Err(ManifestError::InvalidName {
                    name: name.clone(),
                    reason: format!("must start with a lowercase letter, found '{c}'"),
                });
            }
            None => {
                return Err(ManifestError::InvalidName {
                    name: name.clone(),
                    reason: "name cannot be empty".to_string(),
                });
            }
        }

        // Remaining characters must be lowercase letters, digits, or hyphens
        for c in chars {
            if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '-' {
                return Err(ManifestError::InvalidName {
                    name: name.clone(),
                    reason: format!(
                        "invalid character '{c}': only lowercase letters, digits, and hyphens allowed"
                    ),
                });
            }
        }

        Ok(())
    }

    /// Validate the package version as semver.
    fn validate_version(&self) -> Result<(), ManifestError> {
        let version = &self.package.version;

        semver::Version::parse(version).map_err(|e| ManifestError::InvalidVersion {
            version: version.clone(),
            reason: e.to_string(),
        })?;

        Ok(())
    }

    /// Validate the plugin component path.
    fn validate_component_path(&self) -> Result<(), ManifestError> {
        let path_str = &self.plugin.component;
        let path = Path::new(path_str);

        // Must be relative (no leading `/`)
        if path.is_absolute() {
            return Err(ManifestError::InvalidComponentPath {
                path: path_str.clone(),
                reason: "path must be relative".to_string(),
            });
        }

        // Must not contain path escapes (`..`)
        for component in path.components() {
            if component == std::path::Component::ParentDir {
                return Err(ManifestError::InvalidComponentPath {
                    path: path_str.clone(),
                    reason: "path must not contain '..' (parent directory escapes)".to_string(),
                });
            }
        }

        // Should end with .wasm
        if path.extension().is_none_or(|ext| ext != "wasm") {
            return Err(ManifestError::InvalidComponentPath {
                path: path_str.clone(),
                reason: "component must have .wasm extension".to_string(),
            });
        }

        Ok(())
    }

    /// Validate the IR version.
    const fn validate_ir_version(&self) -> Result<(), ManifestError> {
        if self.compatibility.ir_version < 1 {
            return Err(ManifestError::InvalidIrVersion {
                version: self.compatibility.ir_version,
            });
        }
        Ok(())
    }
}

/// Package metadata from the `[package]` section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageMetadata {
    /// Package name (lowercase alphanumeric + hyphens, max 64 chars).
    pub name: String,

    /// Package version (semver).
    pub version: String,

    /// Human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// SPDX license identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,

    /// Source repository URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,

    /// List of authors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
}

/// Plugin specification from the `[plugin]` section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginSpec {
    /// Path to the WASM component within the archive.
    pub component: String,

    /// Service kinds this plugin handles.
    pub service_kinds: Vec<String>,
}

/// Compatibility requirements from the `[compatibility]` section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Compatibility {
    /// Minimum locald version required.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locald_min: Option<String>,

    /// IR version the plugin produces (required, defaults to 1).
    #[serde(default = "default_ir_version")]
    pub ir_version: i32,
}

impl Default for Compatibility {
    fn default() -> Self {
        Self {
            locald_min: None,
            ir_version: default_ir_version(),
        }
    }
}

const fn default_ir_version() -> i32 {
    1
}

/// Capability requirements from the `[capabilities]` section.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityRequirements {
    /// Capabilities the plugin requires to function.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,

    /// Capabilities the plugin can use if granted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optional: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a minimal valid manifest TOML.
    fn minimal_manifest() -> String {
        r#"
[package]
name = "test-plugin"
version = "1.0.0"

[plugin]
component = "plugin.wasm"
service_kinds = ["test"]

[compatibility]
ir_version = 1
"#
        .to_string()
    }

    #[test]
    fn parses_valid_manifest() {
        let toml = minimal_manifest();
        let manifest = PackageManifest::parse(&toml).expect("should parse");

        assert_eq!(manifest.package.name, "test-plugin");
        assert_eq!(manifest.package.version, "1.0.0");
        assert_eq!(manifest.plugin.component, "plugin.wasm");
        assert_eq!(manifest.plugin.service_kinds, vec!["test"]);
        assert_eq!(manifest.compatibility.ir_version, 1);
    }

    #[test]
    fn parses_full_manifest() {
        let toml = r#"
[package]
name = "redis-plugin"
version = "1.0.0"
description = "Redis service support"
license = "MIT"
repository = "https://github.com/example/redis-plugin"
authors = ["Alice <alice@example.com>", "Bob <bob@example.com>"]

[plugin]
component = "plugin.wasm"
service_kinds = ["redis", "redis-cluster"]

[compatibility]
locald_min = "0.2.0"
ir_version = 1

[capabilities]
required = ["oci_pull"]
optional = ["cache_dir", "network"]
"#;
        let manifest = PackageManifest::parse(toml).expect("should parse");

        assert_eq!(manifest.package.name, "redis-plugin");
        assert_eq!(
            manifest.package.description,
            Some("Redis service support".to_string())
        );
        assert_eq!(manifest.package.license, Some("MIT".to_string()));
        assert_eq!(
            manifest.package.repository,
            Some("https://github.com/example/redis-plugin".to_string())
        );
        assert_eq!(manifest.package.authors.len(), 2);

        assert_eq!(
            manifest.plugin.service_kinds,
            vec!["redis", "redis-cluster"]
        );

        assert_eq!(manifest.compatibility.locald_min, Some("0.2.0".to_string()));

        assert_eq!(manifest.capabilities.required, vec!["oci_pull"]);
        assert_eq!(manifest.capabilities.optional, vec!["cache_dir", "network"]);
    }

    #[test]
    fn rejects_empty_name() {
        let toml = r#"
[package]
name = ""
version = "1.0.0"

[plugin]
component = "plugin.wasm"
service_kinds = ["test"]
"#;
        let err = PackageManifest::parse(toml).expect_err("should fail");
        assert!(matches!(err, ManifestError::InvalidName { .. }));
    }

    #[test]
    fn rejects_name_starting_with_number() {
        let toml = r#"
[package]
name = "123plugin"
version = "1.0.0"

[plugin]
component = "plugin.wasm"
service_kinds = ["test"]
"#;
        let err = PackageManifest::parse(toml).expect_err("should fail");
        assert!(matches!(err, ManifestError::InvalidName { .. }));
    }

    #[test]
    fn rejects_name_with_uppercase() {
        let toml = r#"
[package]
name = "MyPlugin"
version = "1.0.0"

[plugin]
component = "plugin.wasm"
service_kinds = ["test"]
"#;
        let err = PackageManifest::parse(toml).expect_err("should fail");
        assert!(matches!(err, ManifestError::InvalidName { .. }));
    }

    #[test]
    fn rejects_name_with_underscore() {
        let toml = r#"
[package]
name = "my_plugin"
version = "1.0.0"

[plugin]
component = "plugin.wasm"
service_kinds = ["test"]
"#;
        let err = PackageManifest::parse(toml).expect_err("should fail");
        assert!(matches!(err, ManifestError::InvalidName { .. }));
    }

    #[test]
    fn rejects_name_exceeding_max_length() {
        let long_name = "a".repeat(65);
        let toml = format!(
            r#"
[package]
name = "{long_name}"
version = "1.0.0"

[plugin]
component = "plugin.wasm"
service_kinds = ["test"]
"#
        );
        let err = PackageManifest::parse(&toml).expect_err("should fail");
        assert!(matches!(err, ManifestError::InvalidName { .. }));
    }

    #[test]
    fn rejects_invalid_semver() {
        let toml = r#"
[package]
name = "test-plugin"
version = "not-a-version"

[plugin]
component = "plugin.wasm"
service_kinds = ["test"]
"#;
        let err = PackageManifest::parse(toml).expect_err("should fail");
        assert!(matches!(err, ManifestError::InvalidVersion { .. }));
    }

    #[test]
    fn rejects_absolute_component_path() {
        let toml = r#"
[package]
name = "test-plugin"
version = "1.0.0"

[plugin]
component = "/etc/passwd"
service_kinds = ["test"]
"#;
        let err = PackageManifest::parse(toml).expect_err("should fail");
        assert!(matches!(err, ManifestError::InvalidComponentPath { .. }));
    }

    #[test]
    fn rejects_path_escape() {
        let toml = r#"
[package]
name = "test-plugin"
version = "1.0.0"

[plugin]
component = "../../../etc/passwd.wasm"
service_kinds = ["test"]
"#;
        let err = PackageManifest::parse(toml).expect_err("should fail");
        assert!(matches!(err, ManifestError::InvalidComponentPath { .. }));
    }

    #[test]
    fn rejects_non_wasm_component() {
        let toml = r#"
[package]
name = "test-plugin"
version = "1.0.0"

[plugin]
component = "plugin.exe"
service_kinds = ["test"]
"#;
        let err = PackageManifest::parse(toml).expect_err("should fail");
        assert!(matches!(err, ManifestError::InvalidComponentPath { .. }));
    }

    #[test]
    fn rejects_zero_ir_version() {
        let toml = r#"
[package]
name = "test-plugin"
version = "1.0.0"

[plugin]
component = "plugin.wasm"
service_kinds = ["test"]

[compatibility]
ir_version = 0
"#;
        let err = PackageManifest::parse(toml).expect_err("should fail");
        assert!(matches!(err, ManifestError::InvalidIrVersion { .. }));
    }

    #[test]
    fn rejects_negative_ir_version() {
        let toml = r#"
[package]
name = "test-plugin"
version = "1.0.0"

[plugin]
component = "plugin.wasm"
service_kinds = ["test"]

[compatibility]
ir_version = -1
"#;
        let err = PackageManifest::parse(toml).expect_err("should fail");
        assert!(matches!(err, ManifestError::InvalidIrVersion { .. }));
    }

    #[test]
    fn defaults_ir_version_to_1() {
        let toml = r#"
[package]
name = "test-plugin"
version = "1.0.0"

[plugin]
component = "plugin.wasm"
service_kinds = ["test"]
"#;
        let manifest = PackageManifest::parse(toml).expect("should parse");
        assert_eq!(manifest.compatibility.ir_version, 1);
    }

    #[test]
    fn accepts_valid_names() {
        let max_len_name = "x".repeat(64);
        let valid_names = [
            "a",
            "my-plugin",
            "redis-v2",
            "postgres123",
            "a-b-c-d-e",
            max_len_name.as_str(),
        ];

        for name in valid_names {
            let toml = format!(
                r#"
[package]
name = "{name}"
version = "1.0.0"

[plugin]
component = "plugin.wasm"
service_kinds = ["test"]
"#
            );
            PackageManifest::parse(&toml)
                .unwrap_or_else(|e| panic!("name '{name}' should be valid: {e}"));
        }
    }

    #[test]
    fn accepts_nested_component_path() {
        let toml = r#"
[package]
name = "test-plugin"
version = "1.0.0"

[plugin]
component = "lib/plugins/my-plugin.wasm"
service_kinds = ["test"]
"#;
        let manifest = PackageManifest::parse(toml).expect("should parse");
        assert_eq!(manifest.plugin.component, "lib/plugins/my-plugin.wasm");
    }

    #[test]
    fn serializes_to_toml() {
        let manifest = PackageManifest {
            package: PackageMetadata {
                name: "test-plugin".to_string(),
                version: "1.0.0".to_string(),
                description: Some("A test plugin".to_string()),
                license: None,
                repository: None,
                authors: vec![],
            },
            plugin: PluginSpec {
                component: "plugin.wasm".to_string(),
                service_kinds: vec!["test".to_string()],
            },
            compatibility: Compatibility {
                locald_min: None,
                ir_version: 1,
            },
            capabilities: CapabilityRequirements::default(),
        };

        let toml_str = toml::to_string_pretty(&manifest).expect("should serialize");
        let reparsed = PackageManifest::parse(&toml_str).expect("should reparse");
        assert_eq!(manifest, reparsed);
    }

    #[test]
    fn missing_package_name_fails() {
        let toml = r#"
[package]
version = "1.0.0"

[plugin]
component = "plugin.wasm"
service_kinds = ["test"]
"#;
        let err = PackageManifest::parse(toml).expect_err("should fail");
        assert!(matches!(err, ManifestError::Parse(_)));
    }

    #[test]
    fn missing_plugin_section_fails() {
        let toml = r#"
[package]
name = "test-plugin"
version = "1.0.0"
"#;
        let err = PackageManifest::parse(toml).expect_err("should fail");
        assert!(matches!(err, ManifestError::Parse(_)));
    }
}
