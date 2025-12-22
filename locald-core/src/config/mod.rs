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
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Root configuration for a locald project.
///
/// # Example
/// ```toml
/// [project]
/// name = "my-app"
///
/// [services.web]
/// command = "npm start"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct LocaldConfig {
    /// Project-level configuration.
    pub project: ProjectConfig,
    /// Service definitions for the project.
    #[serde(default)]
    pub services: HashMap<String, ServiceConfig>,
}

/// Configuration specific to the project identity.
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
/// # Example
/// ```toml
/// [services.web]
/// command = "npm start"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum ServiceConfig {
    /// A typed service configuration (e.g. Postgres, Worker).
    Typed(TypedServiceConfig),
    /// A legacy or simple exec service configuration.
    Legacy(ExecServiceConfig),
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
    /// The interval between checks in seconds.
    #[serde(default)]
    pub interval: Option<u64>,
    /// The timeout for each check in seconds.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// The command to run (for Command probes).
    #[serde(default)]
    pub command: Option<String>,
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

    /// The command to run to start the service. Required if `image` is not set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The Docker image to run. If set, `command` is treated as arguments to the container entrypoint (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// The port exposed by the container. Required if `image` is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_port: Option<u16>,
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
    pub const fn common(&self) -> &CommonServiceConfig {
        match self {
            Self::Typed(TypedServiceConfig::Exec(c)) | Self::Legacy(c) => &c.common,
            Self::Typed(TypedServiceConfig::Postgres(c)) => &c.common,
            Self::Typed(TypedServiceConfig::Worker(c)) => &c.common,
            Self::Typed(TypedServiceConfig::Container(c)) => &c.common,
            Self::Typed(TypedServiceConfig::Site(c)) => &c.common,
        }
    }

    pub const fn port(&self) -> Option<u16> {
        self.common().port
    }

    pub const fn env(&self) -> &HashMap<String, String> {
        &self.common().env
    }

    pub const fn depends_on(&self) -> &Vec<String> {
        &self.common().depends_on
    }

    pub const fn health_check(&self) -> Option<&HealthCheckConfig> {
        self.common().health_check.as_ref()
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
                env: HashMap::new(),
                depends_on: Vec::new(),
                health_check: None,
                stop_signal: None,
            },
            command: Some("echo hello".to_string()),
            workdir: None,
            image: None,
            container_port: None,
            build: None,
        });

        let config = LocaldConfig {
            project: ProjectConfig {
                name: "test-project".to_string(),
                domain: None,
                workspace: None,
                constellation: None,
            },
            services: HashMap::from([("web".to_string(), service_config)]),
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
