//! Project configuration schema for locald.
//!
//! The root config is [`LocaldConfig`], which is composed of three sections:
//!
//! - `[project]` for identity and naming.
//! - `[plugins]` for plugin sources.
//! - `[services.*]` for per-service definitions.
//!
//! # Examples
//! ```toml
//! [project]
//! name = "my-app"
//!
//! [plugins]
//! redis = "https://plugins.locald.dev/redis-plugin-1.0.0.locald-package"
//!
//! [services.web]
//! command = "npm start"
//! ```
//!
//! ```rust
//! use locald_core::config::LocaldConfig;
//!
//! let raw = r#"
//! [project]
//! name = "my-app"
//!
//! [services.web]
//! command = "npm start"
//! "#;
//! let parsed: LocaldConfig = toml::from_str(raw).expect("valid config");
//! assert_eq!(parsed.project.name, "my-app");
//! assert!(parsed.services.contains_key("web"));
//! ```
pub mod global;
pub use global::GlobalConfig;

pub mod env_provenance;
pub use env_provenance::{
    EnvLayer, EnvLayerKind, EnvLayerSource, ResolvedEnv, ResolvedEnvVar, merge_env_layers,
    overlay_env,
};

// FLAG: The `loader` module contains side effects (file I/O, env vars).
// It has been removed from this pure crate.
// pub mod loader;
// pub use loader::{ConfigLoader, Provenance};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;
use std::time::Duration;

/// Root configuration for a locald project.
///
/// This is the primary entry point for parsing `locald.toml`.
///
/// # Example
/// ```toml
/// [project]
/// name = "my-app"
///
/// [plugins]
/// redis = "https://plugins.locald.dev/redis-plugin-1.0.0.locald-package"
///
/// [services.web]
/// command = "npm start"
/// ```
///
/// ```rust
/// use locald_core::config::LocaldConfig;
///
/// let raw = r#"
/// [project]
/// name = "my-app"
///
/// [plugins]
/// redis = "https://plugins.locald.dev/redis-plugin-1.0.0.locald-package"
///
/// [services.web]
/// command = "npm start"
/// "#;
///
/// let config: LocaldConfig = toml::from_str(raw).expect("valid locald config");
/// assert_eq!(config.project.name, "my-app");
/// assert!(config.plugins.contains_key("redis"));
/// assert!(config.services.contains_key("web"));
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct LocaldConfig {
    /// Project-level configuration.
    pub project: ProjectConfig,
    /// Plugin sources for remote or local plugins.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub plugins: HashMap<String, PluginSource>,
    /// Service definitions for the project.
    #[serde(default)]
    pub services: HashMap<String, ServiceConfig>,
    /// Deprecated worktree configuration retained for config compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktrees: Option<WorktreesConfig>,
}

/// Deprecated configuration for branch-derived worktree domains.
///
/// locald now allocates a persistent worktree slug automatically. This shape
/// remains parseable during the compatibility window, but no longer controls
/// domain ownership.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct WorktreesConfig {
    /// Deprecated domain template. Accepted but ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

/// A plugin source reference in locald.toml.
///
/// # Example
/// ```toml
/// [plugins]
/// # Simple URL reference
/// redis = "https://plugins.locald.dev/redis-plugin-1.0.0.locald-package"
///
/// # URL with checksum verification
/// postgres = { url = "https://plugins.locald.dev/postgres-plugin.locald-package", sha256 = "abc123..." }
///
/// # Local path reference (useful for development)
/// custom = { path = "../my-custom-plugin/target/plugin.wasm" }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum PluginSource {
    /// Simple URL string.
    Url(String),

    /// URL with optional checksum.
    Remote {
        /// The URL to fetch the package from.
        url: String,
        /// SHA-256 checksum of the package.
        #[serde(skip_serializing_if = "Option::is_none")]
        sha256: Option<String>,
    },

    /// Local path reference.
    Path {
        /// Path to the plugin WASM file or package.
        path: String,
    },

    /// Reference to an installed plugin (explicit, usually auto-discovered).
    Installed {
        /// Name of the installed plugin.
        installed: String,
    },
}

impl PluginSource {
    /// Check if this is a remote URL source.
    pub const fn is_remote(&self) -> bool {
        matches!(self, Self::Url(_) | Self::Remote { .. })
    }

    /// Get the URL if this is a remote source.
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Url(u) => Some(u),
            Self::Remote { url, .. } => Some(url),
            _ => None,
        }
    }

    /// Get the checksum if present.
    pub fn checksum(&self) -> Option<&str> {
        match self {
            Self::Remote { sha256, .. } => sha256.as_deref(),
            _ => None,
        }
    }

    /// Get the local path if this is a path source.
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Path { path } => Some(path),
            _ => None,
        }
    }

    /// Get the installed plugin name if this is an installed source.
    pub fn installed(&self) -> Option<&str> {
        match self {
            Self::Installed { installed } => Some(installed),
            _ => None,
        }
    }
}

/// Configuration specific to the project identity.
///
/// The `name` is required and influences default domains and identifiers.
///
/// # Example
/// ```toml
/// [project]
/// name = "my-app"
/// domain = "myapp.local"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ProjectConfig {
    /// The name of the project.
    pub name: String,
    /// The domain to serve the project on. Defaults to `{name}.localhost`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// The name of the workspace the project belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// The name of the constellation the project belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constellation: Option<String>,
}

/// Configuration for a single service.
///
/// A service entry can be either a typed service (with a `type = "..."` field)
/// or a legacy exec-style service config. Deserialization checks the
/// discriminator first: once `type` is present, an invalid typed declaration
/// cannot fall through to the permissive legacy exec shape.
///
/// # Example
/// ```toml
/// [services.web]
/// command = "npm start"
/// ```
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum ServiceConfig {
    /// A typed service configuration (e.g. Postgres, Worker).
    Typed(TypedServiceConfig),
    /// A legacy or simple exec service configuration.
    Legacy(ExecServiceConfig),
}

impl<'de> Deserialize<'de> for ServiceConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let is_typed = value
            .as_object()
            .is_some_and(|service| service.contains_key("type"));

        if is_typed {
            serde_json::from_value(value)
                .map(Self::Typed)
                .map_err(D::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(Self::Legacy)
                .map_err(D::Error::custom)
        }
    }
}

/// Enum of supported typed service configurations.
///
/// # Example
/// ```toml
/// [services.db]
/// type = "postgres"
/// version = "15"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TypedServiceConfig {
    /// A generic executable service.
    Exec(ExecServiceConfig),
    /// A managed Postgres database service.
    Postgres(PostgresServiceConfig),
    /// A background worker service.
    Worker(WorkerServiceConfig),
    /// A container-based service.
    Container(ContainerServiceConfig),
    /// A managed site service.
    Site(SiteServiceConfig),
    /// A stable service identity fulfilled by an authenticated external publisher.
    Published(PublishedServiceConfig),
}

/// Configuration for a service whose runtime is owned by an external publisher.
///
/// Published services intentionally expose no process-owned configuration. locald
/// owns their stable domains, HTTP health policy, status, and eventual proxy route,
/// while another same-user process owns the application runtime.
#[derive(Debug, Clone, Default, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublishedServiceConfig {
    /// Optional relative domain claims. Omission preserves the conventional
    /// service-domain mapping; an explicit list must contain an exact claim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domains: Option<Vec<String>>,
    /// Optional HTTP health policy. Omission uses `GET /` with locald defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_check: Option<PublishedHealthCheckConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishedServiceConfigInput {
    domains: Option<Vec<String>>,
    health_check: Option<PublishedHealthCheckConfig>,
}

impl<'de> Deserialize<'de> for PublishedServiceConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let input = PublishedServiceConfigInput::deserialize(deserializer)?;
        if let Some(domains) = &input.domains {
            if domains.is_empty() {
                return Err(D::Error::custom(
                    "a published service's explicit `domains` list cannot be empty",
                ));
            }
            if !domains.iter().any(|domain| !domain.starts_with("*.")) {
                return Err(D::Error::custom(
                    "a published service's explicit `domains` list must contain at least one exact domain claim",
                ));
            }
        }

        Ok(Self {
            domains: input.domains,
            health_check: input.health_check,
        })
    }
}

impl PublishedServiceConfig {
    /// Return the effective HTTP health policy for the declaration.
    #[must_use]
    pub fn normalized_health_check(&self) -> PublishedHealthCheckConfig {
        self.health_check.clone().unwrap_or_default()
    }
}

/// HTTP-only health configuration for one published service.
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublishedHealthCheckConfig {
    /// The probe protocol. Version 1 accepts only HTTP.
    #[serde(rename = "type")]
    pub kind: PublishedProbeType,
    /// Origin-relative path requested by locald. Defaults to `/`.
    #[serde(default = "default_published_health_path")]
    pub path: String,
    /// Seconds between probes. Defaults to one second.
    #[serde(default = "default_published_health_interval")]
    pub interval: u64,
    /// Per-request timeout in seconds. Defaults to five seconds.
    #[serde(default = "default_published_health_timeout")]
    pub timeout: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishedHealthCheckConfigInput {
    #[serde(rename = "type")]
    kind: PublishedProbeType,
    #[serde(default = "default_published_health_path")]
    path: String,
    #[serde(default = "default_published_health_interval")]
    interval: u64,
    #[serde(default = "default_published_health_timeout")]
    timeout: u64,
}

impl<'de> Deserialize<'de> for PublishedHealthCheckConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let input = PublishedHealthCheckConfigInput::deserialize(deserializer)?;
        let path_without_query = input.path.split('?').next().unwrap_or(&input.path);
        if !input.path.starts_with('/')
            || input.path.starts_with("//")
            || input.path.contains('#')
            || input.path.contains(['\r', '\n'])
            || path_without_query
                .split('/')
                .any(is_published_health_dot_segment)
        {
            return Err(D::Error::custom(
                "a published service health-check path must be origin-relative, begin with one `/`, and contain no authority, fragment, or dot segment",
            ));
        }
        if !(1..=60).contains(&input.interval) {
            return Err(D::Error::custom(
                "a published service health-check interval must be between 1 and 60 seconds",
            ));
        }
        if !(1..=10).contains(&input.timeout) {
            return Err(D::Error::custom(
                "a published service health-check timeout must be between 1 and 10 seconds",
            ));
        }

        Ok(Self {
            kind: input.kind,
            path: input.path,
            interval: input.interval,
            timeout: input.timeout,
        })
    }
}

fn is_published_health_dot_segment(segment: &str) -> bool {
    matches!(
        segment.to_ascii_lowercase().as_str(),
        "." | ".." | "%2e" | ".%2e" | "%2e." | "%2e%2e"
    )
}

impl Default for PublishedHealthCheckConfig {
    fn default() -> Self {
        Self {
            kind: PublishedProbeType::Http,
            path: default_published_health_path(),
            interval: default_published_health_interval(),
            timeout: default_published_health_timeout(),
        }
    }
}

impl PublishedHealthCheckConfig {
    /// Return the validated origin-relative probe path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Return the validated probe interval in seconds.
    #[must_use]
    pub const fn interval_secs(&self) -> u64 {
        self.interval
    }

    /// Return the validated per-request timeout in seconds.
    #[must_use]
    pub const fn timeout_secs(&self) -> u64 {
        self.timeout
    }
}

fn default_published_health_path() -> String {
    "/".to_owned()
}

const fn default_published_health_interval() -> u64 {
    DEFAULT_HEALTH_CHECK_INTERVAL_SECS
}

const fn default_published_health_timeout() -> u64 {
    DEFAULT_HEALTH_CHECK_TIMEOUT_SECS
}

/// Probe type accepted by a published service declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum PublishedProbeType {
    /// An application-level HTTP GET.
    #[default]
    Http,
}

/// Configuration for a container-based service.
///
/// # Example
/// ```toml
/// [services.redis]
/// type = "container"
/// image = "redis:7"
/// container_port = 6379
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[schemars(transform = remove_process_runtime_properties)]
pub struct ContainerServiceConfig {
    /// Common configuration shared by all services.
    #[serde(flatten)]
    pub common: CommonServiceConfig,

    /// The Docker image to run.
    pub image: String,
    /// The command to run in the container.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The port exposed by the container.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_port: Option<u16>,
    /// Working directory inside the container.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
}

/// Configuration for a background worker service.
///
/// # Example
/// ```toml
/// [services.worker]
/// type = "worker"
/// command = "bundle exec sidekiq"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct WorkerServiceConfig {
    /// Common configuration shared by all services.
    #[serde(flatten)]
    pub common: CommonServiceConfig,

    /// The command to run to start the worker.
    pub command: String,
    /// Working directory for the command. Defaults to the project root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
}

/// Common configuration fields shared by all service types.
///
/// # Example
/// ```toml
/// port = 3000
/// env = { RAILS_ENV = "development" }
/// depends_on = ["db"]
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CommonServiceConfig {
    /// The port the service listens on. If None, locald will assign a port and pass it via PORT env var.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Additional process-owned listeners that locald allocates dynamically.
    ///
    /// These listeners are private runtime bindings. They do not claim
    /// domains and are exposed to the owning service only through explicit
    /// `${services.<service>.listeners.<listener>.port}` interpolation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub listeners: Vec<String>,
    /// Runtime JSON or JSONC files generated for this service.
    ///
    /// Generated files are materialized in the owning project instance's
    /// locald data directory. Their replacements may reference only this
    /// service's primary port and declared named listeners.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub generated: BTreeMap<String, GeneratedFileConfig>,
    /// Relative domain claims for this service.
    ///
    /// `@` claims the project-instance root, plain values claim exact relative
    /// names, and a leftmost `*.` claims exactly one relative DNS label. When
    /// omitted, locald preserves its conventional service-domain mapping. The
    /// first exact claim is the service's canonical semantic origin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domains: Option<Vec<String>>,
    /// Environment variables to pass to the service.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    /// List of services that must be started before this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Optional command to run to check if the service is healthy.
    /// If not provided, locald will attempt to infer a health check (Docker, Notify, or TCP).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_check: Option<HealthCheckConfig>,
    /// The signal to send to stop the service. Defaults to "SIGTERM".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_signal: Option<String>,
}

/// One JSON or JSONC file generated for a host exec or worker service.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct GeneratedFileConfig {
    /// Project-relative source JSON or JSONC file.
    pub source: String,
    /// Existing JSON Pointer targets and their replacement values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub replace: BTreeMap<String, serde_json::Value>,
}

fn remove_process_runtime_properties(schema: &mut schemars::Schema) {
    if let Some(properties) = schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    {
        properties.remove("listeners");
        properties.remove("generated");
    }
}

/// Configuration for service health checks.
///
/// # Example
/// ```toml
/// health_check = { type = "http", path = "/health" }
/// # OR
/// health_check = "curl -f http://localhost:3000/health"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum HealthCheckConfig {
    /// A shell command to run.
    Command(String),
    /// A structured probe configuration.
    Probe(ProbeConfig),
}

pub const DEFAULT_HEALTH_CHECK_INTERVAL_SECS: u64 = 1;
pub const DEFAULT_HEALTH_CHECK_TIMEOUT_SECS: u64 = 5;

/// Configuration for a health check probe.
///
/// # Example
/// ```toml
/// type = "http"
/// path = "/health"
/// interval = 5
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ProbeConfig {
    /// The type of probe to perform.
    #[serde(rename = "type")]
    pub kind: ProbeType,
    /// The path to check (for HTTP probes).
    #[serde(default)]
    pub path: Option<String>,
    /// The interval between checks in seconds. Defaults to 1 second.
    #[serde(default)]
    pub interval: Option<u64>,
    /// The timeout for each check in seconds. Defaults to 5 seconds.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// The command to run (for Command probes).
    #[serde(default)]
    pub command: Option<String>,
}

impl ProbeConfig {
    pub fn interval_duration(&self) -> Duration {
        Duration::from_secs(self.interval.unwrap_or(DEFAULT_HEALTH_CHECK_INTERVAL_SECS))
    }

    pub fn timeout_duration(&self) -> Duration {
        Duration::from_secs(self.timeout.unwrap_or(DEFAULT_HEALTH_CHECK_TIMEOUT_SECS))
    }
}

/// The type of health check probe.
///
/// # Example
/// ```toml
/// type = "http"
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProbeType {
    /// An HTTP GET request.
    Http,
    /// A TCP connection attempt.
    Tcp,
    /// A shell command execution.
    Command,
}

/// Configuration for a generic executable service.
///
/// # Example
/// ```toml
/// command = "npm start"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ExecServiceConfig {
    /// Common configuration shared by all services.
    #[serde(flatten)]
    pub common: CommonServiceConfig,

    /// The command to run to start the service.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Working directory for the command. Defaults to the project root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Configuration for building the service using CNB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildConfig>,
}

/// Configuration for building a service using Cloud Native Buildpacks.
///
/// # Example
/// ```toml
/// [services.web.build]
/// builder = "heroku/builder:22"
/// buildpacks = ["heroku/nodejs"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct BuildConfig {
    /// The builder image to use. Defaults to "heroku/builder:22".
    #[serde(default = "default_builder")]
    pub builder: String,
    /// List of buildpacks to use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buildpacks: Vec<String>,
}

fn default_builder() -> String {
    "heroku/builder:22".to_string()
}

/// Configuration for a managed Postgres service.
///
/// # Example
/// ```toml
/// type = "postgres"
/// version = "15"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[schemars(transform = remove_process_runtime_properties)]
pub struct PostgresServiceConfig {
    /// Common configuration shared by all services.
    #[serde(flatten)]
    pub common: CommonServiceConfig,

    /// The version of Postgres to use. Defaults to stable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Configuration for a managed site service.
///
/// # Example
/// ```toml
/// [services.docs]
/// type = "site"
/// path = "./docs"
/// build = "cargo doc"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[schemars(transform = remove_process_runtime_properties)]
pub struct SiteServiceConfig {
    /// Common configuration shared by all services.
    #[serde(flatten)]
    pub common: CommonServiceConfig,

    /// The path to the directory to serve.
    pub path: String,
    /// The command to run to build the site.
    #[serde(default)]
    pub build: String,
    /// The name of the service (injected).
    #[serde(skip)]
    pub name: String,
}

impl ServiceConfig {
    /// Return whether this declaration is fulfilled by an external publisher.
    #[must_use]
    pub const fn is_published(&self) -> bool {
        matches!(self, Self::Typed(TypedServiceConfig::Published(_)))
    }

    /// Return the published-service declaration, when this is one.
    #[must_use]
    pub const fn published(&self) -> Option<&PublishedServiceConfig> {
        match self {
            Self::Typed(TypedServiceConfig::Published(config)) => Some(config),
            _ => None,
        }
    }

    /// Return common process configuration for a locald-managed service.
    ///
    /// Published services return an immutable empty process configuration so
    /// read-only compatibility projections remain safe. Runtime dispatch must
    /// still use [`Self::is_published`] to keep them out of controllers.
    pub fn common(&self) -> &CommonServiceConfig {
        static EMPTY: LazyLock<CommonServiceConfig> = LazyLock::new(CommonServiceConfig::default);
        match self {
            Self::Typed(TypedServiceConfig::Exec(c)) | Self::Legacy(c) => &c.common,
            Self::Typed(TypedServiceConfig::Postgres(c)) => &c.common,
            Self::Typed(TypedServiceConfig::Worker(c)) => &c.common,
            Self::Typed(TypedServiceConfig::Container(c)) => &c.common,
            Self::Typed(TypedServiceConfig::Site(c)) => &c.common,
            Self::Typed(TypedServiceConfig::Published(_)) => &EMPTY,
        }
    }

    pub fn port(&self) -> Option<u16> {
        match self {
            Self::Typed(TypedServiceConfig::Published(_)) => None,
            _ => self.common().port,
        }
    }

    pub fn listeners(&self) -> &Vec<String> {
        static EMPTY: Vec<String> = Vec::new();
        match self {
            Self::Typed(TypedServiceConfig::Published(_)) => &EMPTY,
            _ => &self.common().listeners,
        }
    }

    pub fn generated(&self) -> &BTreeMap<String, GeneratedFileConfig> {
        static EMPTY: BTreeMap<String, GeneratedFileConfig> = BTreeMap::new();
        match self {
            Self::Typed(TypedServiceConfig::Published(_)) => &EMPTY,
            _ => &self.common().generated,
        }
    }

    /// Whether this service kind can own dynamically allocated listeners.
    ///
    /// The first listener-backed runtime surface is intentionally limited to
    /// host exec and worker processes. Other service kinds can be added only
    /// when their process/network ownership contract is explicit.
    pub const fn supports_named_listeners(&self) -> bool {
        matches!(
            self,
            Self::Legacy(_)
                | Self::Typed(TypedServiceConfig::Exec(_) | TypedServiceConfig::Worker(_))
        )
    }

    /// Whether this service kind can own host-accessible generated runtime files.
    pub const fn supports_generated_files(&self) -> bool {
        match self {
            Self::Legacy(config) | Self::Typed(TypedServiceConfig::Exec(config)) => {
                config.build.is_none()
            }
            Self::Typed(TypedServiceConfig::Worker(_)) => true,
            Self::Typed(
                TypedServiceConfig::Postgres(_)
                | TypedServiceConfig::Container(_)
                | TypedServiceConfig::Site(_)
                | TypedServiceConfig::Published(_),
            ) => false,
        }
    }

    pub fn env(&self) -> &HashMap<String, String> {
        static EMPTY: LazyLock<HashMap<String, String>> = LazyLock::new(HashMap::new);
        match self {
            Self::Typed(TypedServiceConfig::Published(_)) => &EMPTY,
            _ => &self.common().env,
        }
    }

    pub fn domains(&self) -> Option<&[String]> {
        match self {
            Self::Typed(TypedServiceConfig::Published(config)) => config.domains.as_deref(),
            _ => self.common().domains.as_deref(),
        }
    }

    /// Return whether two service definitions require the same runtime.
    ///
    /// Domain aliases and wildcard claims are proxy ownership. A changed
    /// canonical origin still changes any resolved `${services.*.origin}`
    /// environment and is therefore detected separately by the manager.
    #[must_use]
    pub fn runtime_eq(&self, other: &Self) -> bool {
        if self.is_published() || other.is_published() {
            return self.is_published() && other.is_published();
        }
        let mut left = self.clone();
        let mut right = other.clone();
        if let Some(common) = left.common_mut() {
            common.domains = None;
        }
        if let Some(common) = right.common_mut() {
            common.domains = None;
        }
        left == right
    }

    pub fn depends_on(&self) -> &Vec<String> {
        static EMPTY: Vec<String> = Vec::new();
        match self {
            Self::Typed(TypedServiceConfig::Published(_)) => &EMPTY,
            _ => &self.common().depends_on,
        }
    }

    pub fn health_check(&self) -> Option<&HealthCheckConfig> {
        match self {
            Self::Typed(TypedServiceConfig::Published(_)) => None,
            _ => self.common().health_check.as_ref(),
        }
    }

    const fn common_mut(&mut self) -> Option<&mut CommonServiceConfig> {
        match self {
            Self::Typed(TypedServiceConfig::Exec(c)) | Self::Legacy(c) => Some(&mut c.common),
            Self::Typed(TypedServiceConfig::Postgres(c)) => Some(&mut c.common),
            Self::Typed(TypedServiceConfig::Worker(c)) => Some(&mut c.common),
            Self::Typed(TypedServiceConfig::Container(c)) => Some(&mut c.common),
            Self::Typed(TypedServiceConfig::Site(c)) => Some(&mut c.common),
            Self::Typed(TypedServiceConfig::Published(_)) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialization_skips_empty_fields() {
        let service_config = ServiceConfig::Legacy(ExecServiceConfig {
            common: CommonServiceConfig {
                port: None,
                listeners: Vec::new(),
                generated: BTreeMap::new(),
                domains: None,
                env: HashMap::new(),
                depends_on: Vec::new(),
                health_check: None,
                stop_signal: None,
            },
            command: Some("echo hello".to_string()),
            workdir: None,
            build: None,
        });

        let config = LocaldConfig {
            project: ProjectConfig {
                name: "test-project".to_string(),
                domain: None,
                workspace: None,
                constellation: None,
            },
            plugins: HashMap::new(),
            services: HashMap::from([("web".to_string(), service_config)]),
            worktrees: None,
        };

        let toml_string = toml::to_string_pretty(&config).unwrap();

        // Check that empty fields are NOT present
        assert!(!toml_string.contains("workdir"));
        assert!(!toml_string.contains("env"));
        assert!(!toml_string.contains("depends_on"));
        assert!(!toml_string.contains("image"));
        assert!(!toml_string.contains("container_port"));
        assert!(!toml_string.contains("health_check"));
        assert!(!toml_string.contains("domain"));

        // Check that present fields ARE present
        assert!(toml_string.contains("command = \"echo hello\""));
        assert!(toml_string.contains("[project]"));
        assert!(toml_string.contains("name = \"test-project\""));
    }

    #[test]
    fn runtime_equality_ignores_only_domain_claims() {
        let mut current: ServiceConfig = toml::from_str(
            r#"
command = "serve"
domains = ["frame", "*.frame"]
"#,
        )
        .expect("parse current service");
        let mut candidate = current.clone();
        candidate
            .common_mut()
            .expect("managed service common config")
            .domains = Some(
            ["frame", "preview"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        );

        assert!(current.runtime_eq(&candidate));

        current
            .common_mut()
            .expect("managed service common config")
            .env
            .insert("MODE".into(), "old".into());
        candidate
            .common_mut()
            .expect("managed service common config")
            .env
            .insert("MODE".into(), "new".into());
        assert!(!current.runtime_eq(&candidate));
    }

    #[test]
    fn named_listeners_round_trip_and_change_runtime_identity() {
        let config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "listener-app"

[services.web]
command = "serve"
listeners = ["chat", "hmr-control"]
"#,
        )
        .expect("parse named listener config");
        let service = config.services.get("web").expect("web service");
        assert_eq!(service.listeners(), &["chat", "hmr-control"]);

        let encoded = toml::to_string(&config).expect("serialize named listener config");
        let round_tripped: LocaldConfig =
            toml::from_str(&encoded).expect("round-trip named listener config");
        assert_eq!(
            round_tripped
                .services
                .get("web")
                .expect("round-tripped web service")
                .listeners(),
            &["chat", "hmr-control"]
        );

        let without_listeners: ServiceConfig =
            toml::from_str("command = \"serve\"").expect("parse plain service");
        assert!(!service.runtime_eq(&without_listeners));
    }

    #[test]
    fn generated_files_round_trip_with_typed_replacements() {
        let config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "generated-app"

[services.web]
command = "serve"
listeners = ["chat"]

[services.web.generated.microfrontends]
source = "chat/microfrontends.jsonc"

[services.web.generated.microfrontends.replace]
"/applications/chat/development/local" = "${services.web.listeners.chat.port}"
"/options/enabled" = true
"#,
        )
        .expect("parse generated file config");
        let generated = &config.services["web"].generated()["microfrontends"];
        assert_eq!(generated.source, "chat/microfrontends.jsonc");
        assert_eq!(
            generated.replace["/options/enabled"],
            serde_json::Value::Bool(true)
        );

        let encoded = toml::to_string(&config).expect("serialize generated file config");
        let round_tripped: LocaldConfig =
            toml::from_str(&encoded).expect("round-trip generated file config");
        assert_eq!(round_tripped, config);

        let without_generated: ServiceConfig =
            toml::from_str("command = \"serve\"").expect("parse plain service");
        assert!(!config.services["web"].runtime_eq(&without_generated));
    }

    #[test]
    fn process_runtime_schema_matches_supported_service_kinds() {
        let schema =
            serde_json::to_value(schemars::schema_for!(LocaldConfig)).expect("serialize schema");

        for supported in ["ExecServiceConfig", "WorkerServiceConfig"] {
            for property in ["listeners", "generated"] {
                assert!(
                    schema
                        .pointer(&format!("/$defs/{supported}/properties/{property}"))
                        .is_some(),
                    "{supported} should expose {property}"
                );
            }
        }

        for unsupported in [
            "ContainerServiceConfig",
            "PostgresServiceConfig",
            "SiteServiceConfig",
        ] {
            for property in ["listeners", "generated"] {
                assert!(
                    schema
                        .pointer(&format!("/$defs/{unsupported}/properties/{property}"))
                        .is_none(),
                    "{unsupported} must not expose {property}"
                );
            }
        }

        let published = schema
            .pointer("/$defs/TypedServiceConfig/oneOf")
            .and_then(serde_json::Value::as_array)
            .and_then(|variants| {
                variants.iter().find(|variant| {
                    variant.pointer("/properties/type/const")
                        == Some(&serde_json::Value::String("published".to_owned()))
                })
            })
            .expect("published service schema variant");
        assert_eq!(
            published.pointer("/additionalProperties"),
            Some(&serde_json::Value::Bool(false))
        );
        assert_eq!(
            published
                .pointer("/properties")
                .and_then(serde_json::Value::as_object)
                .expect("published properties")
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["domains", "health_check", "type"]
        );

        let published_health = schema
            .pointer("/$defs/PublishedHealthCheckConfig")
            .expect("published health schema");
        assert_eq!(
            published_health.pointer("/additionalProperties"),
            Some(&serde_json::Value::Bool(false))
        );
        assert_eq!(
            published_health.pointer("/required"),
            Some(&serde_json::json!(["type"]))
        );
    }

    #[test]
    fn published_service_round_trips_with_normalized_http_defaults() {
        let config: LocaldConfig = toml::from_str(
            r#"
[project]
name = "published-app"

[services.workbench]
type = "published"
"#,
        )
        .expect("parse minimal published service");
        let service = config.services.get("workbench").expect("workbench");
        let published = service.published().expect("published declaration");
        assert_eq!(published.domains, None);
        assert_eq!(
            published.normalized_health_check(),
            PublishedHealthCheckConfig::default()
        );
        assert!(service.is_published());
        assert_eq!(service.port(), None);
        assert!(service.listeners().is_empty());
        assert!(service.generated().is_empty());
        assert!(service.env().is_empty());
        assert!(service.depends_on().is_empty());
        assert!(service.health_check().is_none());

        let encoded = toml::to_string(&config).expect("serialize published config");
        let round_tripped: LocaldConfig =
            toml::from_str(&encoded).expect("round-trip published config");
        assert_eq!(round_tripped, config);
        assert!(encoded.contains("type = \"published\""));
        assert!(!encoded.contains("health_check"));
    }

    #[test]
    fn published_service_accepts_only_normalized_http_health() {
        let service: ServiceConfig = toml::from_str(
            r#"
type = "published"
domains = ["workbench", "*.workbench"]

[health_check]
type = "http"
path = "/ready?full=true"
interval = 60
timeout = 10
"#,
        )
        .expect("parse explicit published health policy");
        let published = service.published().expect("published declaration");
        assert_eq!(
            published.domains.as_deref(),
            Some(["workbench".to_owned(), "*.workbench".to_owned()].as_slice())
        );
        let health = published.normalized_health_check();
        assert_eq!(health.kind, PublishedProbeType::Http);
        assert_eq!(health.path(), "/ready?full=true");
        assert_eq!(health.interval_secs(), 60);
        assert_eq!(health.timeout_secs(), 10);

        for invalid in [
            "type = \"published\"\ndomains = []",
            "type = \"published\"\ndomains = [\"*.workbench\"]",
            "type = \"published\"\nhealth_check = \"curl /\"",
            "type = \"published\"\nhealth_check = { type = \"tcp\" }",
            "type = \"published\"\nhealth_check = { type = \"http\", path = \"ready\" }",
            "type = \"published\"\nhealth_check = { type = \"http\", path = \"//other.test/ready\" }",
            "type = \"published\"\nhealth_check = { type = \"http\", path = \"/a/../ready\" }",
            "type = \"published\"\nhealth_check = { type = \"http\", path = \"/a/%2E%2e/ready\" }",
            "type = \"published\"\nhealth_check = { type = \"http\", path = \"/ready#fragment\" }",
            "type = \"published\"\nhealth_check = { type = \"http\", path = \"/ready\\nnext\" }",
            "type = \"published\"\nhealth_check = { type = \"http\", command = \"curl /\" }",
            "type = \"published\"\nhealth_check = { type = \"http\", interval = 0 }",
            "type = \"published\"\nhealth_check = { type = \"http\", interval = 61 }",
            "type = \"published\"\nhealth_check = { type = \"http\", timeout = 0 }",
            "type = \"published\"\nhealth_check = { type = \"http\", timeout = 11 }",
        ] {
            assert!(
                toml::from_str::<ServiceConfig>(invalid).is_err(),
                "invalid published health/domain config was accepted: {invalid}"
            );
        }
    }

    #[test]
    fn published_service_cannot_fall_through_to_legacy_exec() {
        for field in [
            "command = \"serve\"",
            "workdir = \"app\"",
            "build = { builder = \"builder\" }",
            "env = { MODE = \"dev\" }",
            "port = 3000",
            "listeners = [\"hmr\"]",
            "generated = {}",
            "depends_on = [\"db\"]",
            "stop_signal = \"SIGINT\"",
            "unexpected = true",
        ] {
            let raw = format!("type = \"published\"\n{field}");
            let error = toml::from_str::<ServiceConfig>(&raw)
                .expect_err("published process/unknown field must fail");
            assert!(
                error.to_string().contains("unknown field"),
                "unexpected error for {field}: {error}"
            );
        }
    }

    #[test]
    fn typed_service_errors_never_fall_through_to_legacy_exec() {
        let error =
            toml::from_str::<ServiceConfig>("type = \"not-a-runtime\"\ncommand = \"serve\"")
                .expect_err("unknown typed runtime must fail");
        assert!(error.to_string().contains("unknown variant"));
    }

    #[test]
    fn test_postgres_config() {
        let toml = r#"
[project]
name = "pg-test"

[services.db]
type = "postgres"
version = "15"
"#;
        let config: LocaldConfig = toml::from_str(toml).unwrap();
        let service = config.services.get("db").unwrap();

        match service {
            ServiceConfig::Typed(TypedServiceConfig::Postgres(pg)) => {
                assert_eq!(pg.version.as_deref(), Some("15"));
            }
            _ => panic!("Expected Postgres config"),
        }
    }

    #[test]
    fn test_health_check_config() {
        let toml = r#"
[project]
name = "hc-test"

[services.web]
command = "cmd"
health_check = { type = "http", path = "/" }

[services.worker]
type = "worker"
command = "cmd"
health_check = "test -f ready"
"#;
        let config: LocaldConfig = toml::from_str(toml).unwrap();
        let service = config.services.get("web").unwrap();

        if let Some(HealthCheckConfig::Probe(probe)) = service.health_check() {
            assert_eq!(probe.kind, ProbeType::Http);
            assert_eq!(probe.path.as_deref(), Some("/"));
        } else {
            panic!("Expected Probe config");
        }
    }

    #[test]
    fn test_plugin_source_url() {
        let toml = r#"
[project]
name = "plugin-test"

[plugins]
redis = "https://plugins.locald.dev/redis-plugin-1.0.0.locald-package"
"#;
        let config: LocaldConfig = toml::from_str(toml).unwrap();
        let plugin = config.plugins.get("redis").unwrap();

        assert!(plugin.is_remote());
        assert_eq!(
            plugin.url(),
            Some("https://plugins.locald.dev/redis-plugin-1.0.0.locald-package")
        );
        assert!(plugin.checksum().is_none());
    }

    #[test]
    fn test_plugin_source_with_checksum() {
        let toml = r#"
[project]
name = "plugin-test"

[plugins]
postgres = { url = "https://example.com/plugin.locald-package", sha256 = "abc123" }
"#;
        let config: LocaldConfig = toml::from_str(toml).unwrap();
        let plugin = config.plugins.get("postgres").unwrap();

        assert!(plugin.is_remote());
        assert_eq!(
            plugin.url(),
            Some("https://example.com/plugin.locald-package")
        );
        assert_eq!(plugin.checksum(), Some("abc123"));
    }

    #[test]
    fn test_plugin_source_path() {
        let toml = r#"
[project]
name = "plugin-test"

[plugins]
custom = { path = "../my-plugin/target/plugin.wasm" }
"#;
        let config: LocaldConfig = toml::from_str(toml).unwrap();
        let plugin = config.plugins.get("custom").unwrap();

        assert!(!plugin.is_remote());
        assert_eq!(plugin.path(), Some("../my-plugin/target/plugin.wasm"));
    }

    #[test]
    fn test_plugin_source_installed() {
        let toml = r#"
[project]
name = "plugin-test"

[plugins]
sidekiq = { installed = "sidekiq-plugin" }
"#;
        let config: LocaldConfig = toml::from_str(toml).unwrap();
        let plugin = config.plugins.get("sidekiq").unwrap();

        assert!(!plugin.is_remote());
        assert_eq!(plugin.installed(), Some("sidekiq-plugin"));
    }
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            domain: None,
            workspace: None,
            constellation: None,
        }
    }
}
