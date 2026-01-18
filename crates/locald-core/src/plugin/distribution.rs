//! Distribution manifest schema for `.locald-distribution` files.
//!
//! This module defines the distribution format specified in RFC 0129, section 3.14.
//! The distribution is a TOML file (`distribution.toml`) that describes a project bootstrap kit.
//!
//! # Example
//!
//! ```toml
//! [distribution]
//! name = "redis-stack"
//! version = "1.0.0"
//! description = "Redis + Postgres dev stack"
//!
//! [compatibility]
//! locald_min = "0.2.0"
//!
//! [plugins]
//! bundled = ["redis-plugin-1.0.0.locald-package"]
//! remote = ["https://plugins.locald.dev/postgres-plugin.locald-package"]
//!
//! [scaffold]
//! templates = ["README.md.template"]
//! files = [".gitignore"]
//!
//! [scaffold.variables]
//! project_name = { prompt = "Project name", default = "my-project" }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// Maximum length for distribution names.
const MAX_NAME_LENGTH: usize = 64;

/// Errors that can occur when parsing or validating a distribution manifest.
#[derive(Debug, Error)]
pub enum DistributionError {
    /// Failed to parse the manifest TOML.
    #[error("failed to parse distribution manifest: {0}")]
    Parse(#[from] toml::de::Error),

    /// Distribution name is invalid.
    #[error("invalid distribution name '{name}': {reason}")]
    InvalidName {
        /// The invalid name.
        name: String,
        /// Why the name is invalid.
        reason: String,
    },

    /// Distribution version is not valid semver.
    #[error("invalid distribution version '{version}': {reason}")]
    InvalidVersion {
        /// The invalid version string.
        version: String,
        /// Why the version is invalid.
        reason: String,
    },

    /// A bundled plugin path is invalid.
    #[error("invalid bundled plugin path '{path}': {reason}")]
    InvalidBundledPath {
        /// The invalid path.
        path: String,
        /// Why the path is invalid.
        reason: String,
    },

    /// A remote plugin URL is invalid.
    #[error("invalid remote plugin URL '{url}': {reason}")]
    InvalidRemoteUrl {
        /// The invalid URL.
        url: String,
        /// Why the URL is invalid.
        reason: String,
    },

    /// A template file path is invalid.
    #[error("invalid template path '{path}': {reason}")]
    InvalidTemplatePath {
        /// The invalid path.
        path: String,
        /// Why the path is invalid.
        reason: String,
    },

    /// A scaffold variable name is invalid.
    #[error("invalid variable name '{name}': {reason}")]
    InvalidVariableName {
        /// The invalid name.
        name: String,
        /// Why the name is invalid.
        reason: String,
    },

    /// Missing required field.
    #[error("missing required field: {field}")]
    MissingField {
        /// The missing field name.
        field: String,
    },

    /// Template syntax error.
    #[error("template syntax error in '{file}': {reason}")]
    TemplateSyntaxError {
        /// The file with the error.
        file: String,
        /// Description of the error.
        reason: String,
    },
}

/// Root structure for a distribution manifest.
///
/// Corresponds to the `distribution.toml` file in a `.locald-distribution` archive.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DistributionManifest {
    /// Distribution metadata (`[distribution]` section).
    pub distribution: DistributionMetadata,

    /// Compatibility requirements (`[compatibility]` section).
    #[serde(default)]
    pub compatibility: DistributionCompatibility,

    /// Plugin configuration (`[plugins]` section).
    #[serde(default)]
    pub plugins: DistributionPlugins,

    /// Scaffold configuration (`[scaffold]` section).
    #[serde(default)]
    pub scaffold: DistributionScaffold,
}

impl DistributionManifest {
    /// Parse a manifest from TOML string content.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing fails or validation fails.
    pub fn parse(content: &str) -> Result<Self, DistributionError> {
        let manifest: Self = toml::from_str(content)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate all fields of the manifest.
    ///
    /// # Errors
    ///
    /// Returns the first validation error encountered.
    pub fn validate(&self) -> Result<(), DistributionError> {
        self.validate_name()?;
        self.validate_version()?;
        self.validate_bundled_plugins()?;
        self.validate_remote_plugins()?;
        self.validate_templates()?;
        self.validate_variables()?;
        Ok(())
    }

    /// Validate the distribution name.
    fn validate_name(&self) -> Result<(), DistributionError> {
        let name = &self.distribution.name;

        if name.is_empty() {
            return Err(DistributionError::InvalidName {
                name: name.clone(),
                reason: "name cannot be empty".to_string(),
            });
        }

        if name.len() > MAX_NAME_LENGTH {
            return Err(DistributionError::InvalidName {
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
                return Err(DistributionError::InvalidName {
                    name: name.clone(),
                    reason: format!("must start with a lowercase letter, found '{c}'"),
                });
            }
            None => {
                return Err(DistributionError::InvalidName {
                    name: name.clone(),
                    reason: "name cannot be empty".to_string(),
                });
            }
        }

        // Remaining characters must be lowercase letters, digits, or hyphens
        for c in chars {
            if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '-' {
                return Err(DistributionError::InvalidName {
                    name: name.clone(),
                    reason: format!(
                        "invalid character '{c}': only lowercase letters, digits, and hyphens allowed"
                    ),
                });
            }
        }

        Ok(())
    }

    /// Validate the distribution version as semver.
    fn validate_version(&self) -> Result<(), DistributionError> {
        let version = &self.distribution.version;

        semver::Version::parse(version).map_err(|e| DistributionError::InvalidVersion {
            version: version.clone(),
            reason: e.to_string(),
        })?;

        Ok(())
    }

    /// Validate bundled plugin paths.
    fn validate_bundled_plugins(&self) -> Result<(), DistributionError> {
        for path in &self.plugins.bundled {
            // Must end with .locald-package
            if !path.ends_with(".locald-package") {
                return Err(DistributionError::InvalidBundledPath {
                    path: path.clone(),
                    reason: "must end with .locald-package".to_string(),
                });
            }

            // Must not contain path traversal
            if path.contains("..") {
                return Err(DistributionError::InvalidBundledPath {
                    path: path.clone(),
                    reason: "path must not contain '..'".to_string(),
                });
            }

            // Must not be absolute
            if path.starts_with('/') {
                return Err(DistributionError::InvalidBundledPath {
                    path: path.clone(),
                    reason: "path must be relative".to_string(),
                });
            }
        }

        Ok(())
    }

    /// Validate remote plugin references.
    fn validate_remote_plugins(&self) -> Result<(), DistributionError> {
        for remote in &self.plugins.remote {
            let url = match remote {
                RemotePluginRef::Url(u) => u,
                RemotePluginRef::WithChecksum { url, .. } => url,
            };

            // Must be http(s) URL
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(DistributionError::InvalidRemoteUrl {
                    url: url.clone(),
                    reason: "must be an http:// or https:// URL".to_string(),
                });
            }

            // Must end with .locald-package
            let url_without_query = url.split('?').next().unwrap_or(url);
            if !url_without_query.ends_with(".locald-package") {
                return Err(DistributionError::InvalidRemoteUrl {
                    url: url.clone(),
                    reason: "URL must end with .locald-package".to_string(),
                });
            }
        }

        Ok(())
    }

    /// Validate template file paths.
    fn validate_templates(&self) -> Result<(), DistributionError> {
        for path in &self.scaffold.templates {
            // Must not contain path traversal
            if path.contains("..") {
                return Err(DistributionError::InvalidTemplatePath {
                    path: path.clone(),
                    reason: "path must not contain '..'".to_string(),
                });
            }

            // Must not be absolute
            if path.starts_with('/') {
                return Err(DistributionError::InvalidTemplatePath {
                    path: path.clone(),
                    reason: "path must be relative".to_string(),
                });
            }
        }

        for path in &self.scaffold.files {
            // Must not contain path traversal
            if path.contains("..") {
                return Err(DistributionError::InvalidTemplatePath {
                    path: path.clone(),
                    reason: "path must not contain '..'".to_string(),
                });
            }

            // Must not be absolute
            if path.starts_with('/') {
                return Err(DistributionError::InvalidTemplatePath {
                    path: path.clone(),
                    reason: "path must be relative".to_string(),
                });
            }
        }

        Ok(())
    }

    /// Validate scaffold variable names.
    fn validate_variables(&self) -> Result<(), DistributionError> {
        for name in self.scaffold.variables.keys() {
            // Must be valid identifier (alphanumeric + underscore, start with letter)
            if name.is_empty() {
                return Err(DistributionError::InvalidVariableName {
                    name: name.clone(),
                    reason: "variable name cannot be empty".to_string(),
                });
            }

            let mut chars = name.chars();
            match chars.next() {
                Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
                Some(c) => {
                    return Err(DistributionError::InvalidVariableName {
                        name: name.clone(),
                        reason: format!("must start with a letter or underscore, found '{c}'"),
                    });
                }
                None => unreachable!(), // Already checked is_empty
            }

            for c in chars {
                if !c.is_ascii_alphanumeric() && c != '_' {
                    return Err(DistributionError::InvalidVariableName {
                        name: name.clone(),
                        reason: format!(
                            "invalid character '{c}': only letters, digits, and underscores allowed"
                        ),
                    });
                }
            }
        }

        Ok(())
    }

    /// Get the default output filename for this distribution.
    pub fn default_filename(&self) -> String {
        format!(
            "{}-{}.locald-distribution",
            self.distribution.name, self.distribution.version
        )
    }
}

/// Distribution metadata from the `[distribution]` section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DistributionMetadata {
    /// Distribution name (lowercase alphanumeric + hyphens, max 64 chars).
    pub name: String,

    /// Distribution version (semver).
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

/// Compatibility requirements from the `[compatibility]` section.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DistributionCompatibility {
    /// Minimum locald version required.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locald_min: Option<String>,
}

/// Plugin configuration from the `[plugins]` section.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DistributionPlugins {
    /// Bundled plugin package files (relative paths in packages/ directory).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bundled: Vec<String>,

    /// Remote plugin references (URLs, optionally with checksums).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote: Vec<RemotePluginRef>,
}

/// A reference to a remote plugin package.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum RemotePluginRef {
    /// Simple URL string.
    Url(String),

    /// URL with optional checksum for verification.
    WithChecksum {
        /// The URL to fetch the package from.
        url: String,
        /// SHA-256 checksum of the package (optional).
        #[serde(skip_serializing_if = "Option::is_none")]
        sha256: Option<String>,
    },
}

impl RemotePluginRef {
    /// Get the URL of this remote plugin reference.
    pub fn url(&self) -> &str {
        match self {
            Self::Url(u) => u,
            Self::WithChecksum { url, .. } => url,
        }
    }

    /// Get the checksum if present.
    pub fn checksum(&self) -> Option<&str> {
        match self {
            Self::Url(_) => None,
            Self::WithChecksum { sha256, .. } => sha256.as_deref(),
        }
    }
}

/// Scaffold configuration from the `[scaffold]` section.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DistributionScaffold {
    /// Template files to render with variable substitution.
    /// Output filename removes `.template` suffix.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub templates: Vec<String>,

    /// Files to copy as-is without modification.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,

    /// Variables available for template substitution.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub variables: HashMap<String, ScaffoldVariable>,
}

/// Configuration for a scaffold variable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScaffoldVariable {
    /// Prompt text shown to the user.
    pub prompt: String,

    /// Default value if user doesn't provide one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Render a template string with variable substitution.
///
/// Variables use `{{variable_name}}` syntax.
///
/// # Arguments
///
/// * `template` - The template string to render
/// * `variables` - Map of variable names to their values
///
/// # Returns
///
/// The rendered string, or an error if template syntax is invalid.
pub fn render_template<S: std::hash::BuildHasher>(
    template: &str,
    variables: &HashMap<String, String, S>,
) -> Result<String, DistributionError> {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '{' && chars.peek() == Some(&'{') {
            chars.next(); // consume second '{'

            // Read variable name
            let mut var_name = String::new();
            loop {
                match chars.next() {
                    Some('}') if chars.peek() == Some(&'}') => {
                        chars.next(); // consume second '}'
                        break;
                    }
                    Some(ch) => var_name.push(ch),
                    None => {
                        return Err(DistributionError::TemplateSyntaxError {
                            file: "template".to_string(),
                            reason: "unclosed variable reference '{{' without '}}'".to_string(),
                        });
                    }
                }
            }

            let var_name = var_name.trim();
            if let Some(value) = variables.get(var_name) {
                result.push_str(value);
            } else {
                // Leave unresolved variables as-is (or could error)
                result.push_str("{{");
                result.push_str(var_name);
                result.push_str("}}");
            }
        } else {
            result.push(c);
        }
    }

    Ok(result)
}

/// Validate that a template has balanced `{{` and `}}`.
pub fn validate_template_syntax(content: &str, filename: &str) -> Result<(), DistributionError> {
    let mut depth = 0;
    let mut chars = content.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '{' && chars.peek() == Some(&'{') {
            chars.next();
            depth += 1;
        } else if c == '}' && chars.peek() == Some(&'}') {
            chars.next();
            if depth == 0 {
                return Err(DistributionError::TemplateSyntaxError {
                    file: filename.to_string(),
                    reason: "unmatched '}}' found".to_string(),
                });
            }
            depth -= 1;
        }
    }

    if depth != 0 {
        return Err(DistributionError::TemplateSyntaxError {
            file: filename.to_string(),
            reason: format!("{depth} unclosed '{{{{' found"),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_manifest() -> String {
        r#"
[distribution]
name = "test-dist"
version = "1.0.0"
"#
        .to_string()
    }

    #[test]
    fn parses_minimal_manifest() {
        let manifest = DistributionManifest::parse(&minimal_manifest()).expect("should parse");
        assert_eq!(manifest.distribution.name, "test-dist");
        assert_eq!(manifest.distribution.version, "1.0.0");
    }

    #[test]
    fn parses_full_manifest() {
        let toml = r#"
[distribution]
name = "redis-stack"
version = "1.0.0"
description = "Redis + Postgres dev stack"
license = "MIT"
repository = "https://github.com/example/redis-stack"
authors = ["Alice <alice@example.com>"]

[compatibility]
locald_min = "0.2.0"

[plugins]
bundled = ["redis-plugin-1.0.0.locald-package"]
remote = [
    "https://plugins.locald.dev/postgres-plugin-2.0.0.locald-package",
    { url = "https://internal.co/custom.locald-package", sha256 = "abc123" }
]

[scaffold]
templates = ["README.md.template"]
files = [".gitignore", "docker-compose.yml"]

[scaffold.variables]
project_name = { prompt = "Project name", default = "my-project" }
database_name = { prompt = "Database name" }
"#;
        let manifest = DistributionManifest::parse(toml).expect("should parse");

        assert_eq!(manifest.distribution.name, "redis-stack");
        assert_eq!(
            manifest.distribution.description,
            Some("Redis + Postgres dev stack".to_string())
        );
        assert_eq!(manifest.compatibility.locald_min, Some("0.2.0".to_string()));
        assert_eq!(manifest.plugins.bundled.len(), 1);
        assert_eq!(manifest.plugins.remote.len(), 2);
        assert_eq!(manifest.scaffold.templates.len(), 1);
        assert_eq!(manifest.scaffold.files.len(), 2);
        assert_eq!(manifest.scaffold.variables.len(), 2);
    }

    #[test]
    fn rejects_empty_name() {
        let toml = r#"
[distribution]
name = ""
version = "1.0.0"
"#;
        let err = DistributionManifest::parse(toml).expect_err("should fail");
        assert!(matches!(err, DistributionError::InvalidName { .. }));
    }

    #[test]
    fn rejects_invalid_bundled_path() {
        let toml = r#"
[distribution]
name = "test"
version = "1.0.0"

[plugins]
bundled = ["../escape.locald-package"]
"#;
        let err = DistributionManifest::parse(toml).expect_err("should fail");
        assert!(matches!(err, DistributionError::InvalidBundledPath { .. }));
    }

    #[test]
    fn rejects_invalid_remote_url() {
        let toml = r#"
[distribution]
name = "test"
version = "1.0.0"

[plugins]
remote = ["file:///local/path.locald-package"]
"#;
        let err = DistributionManifest::parse(toml).expect_err("should fail");
        assert!(matches!(err, DistributionError::InvalidRemoteUrl { .. }));
    }

    #[test]
    fn test_render_template() {
        let template = "Hello, {{name}}! Welcome to {{project}}.";
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "Alice".to_string());
        vars.insert("project".to_string(), "locald".to_string());

        let result = render_template(template, &vars).expect("should render");
        assert_eq!(result, "Hello, Alice! Welcome to locald.");
    }

    #[test]
    fn test_render_template_missing_var() {
        let template = "Hello, {{name}}!";
        let vars = HashMap::new();

        let result = render_template(template, &vars).expect("should render");
        assert_eq!(result, "Hello, {{name}}!");
    }

    #[test]
    fn test_validate_template_syntax_valid() {
        let content = "Hello {{name}}, your project is {{project_name}}.";
        validate_template_syntax(content, "test.txt").expect("should be valid");
    }

    #[test]
    fn test_validate_template_syntax_unclosed() {
        let content = "Hello {{name}, incomplete.";
        let err = validate_template_syntax(content, "test.txt").expect_err("should fail");
        assert!(matches!(err, DistributionError::TemplateSyntaxError { .. }));
    }

    #[test]
    fn test_default_filename() {
        let manifest = DistributionManifest::parse(&minimal_manifest()).unwrap();
        assert_eq!(
            manifest.default_filename(),
            "test-dist-1.0.0.locald-distribution"
        );
    }

    #[test]
    fn test_remote_plugin_ref_accessors() {
        let url_ref = RemotePluginRef::Url("https://example.com/plugin.locald-package".to_string());
        assert_eq!(url_ref.url(), "https://example.com/plugin.locald-package");
        assert!(url_ref.checksum().is_none());

        let checksum_ref = RemotePluginRef::WithChecksum {
            url: "https://example.com/plugin.locald-package".to_string(),
            sha256: Some("abc123".to_string()),
        };
        assert_eq!(
            checksum_ref.url(),
            "https://example.com/plugin.locald-package"
        );
        assert_eq!(checksum_ref.checksum(), Some("abc123"));
    }
}
