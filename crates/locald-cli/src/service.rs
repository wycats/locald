use crate::error::CliResult;
use anyhow::Context;
use locald_core::config::{
    CommonServiceConfig, ContainerServiceConfig, ExecServiceConfig, LocaldConfig,
    PostgresServiceConfig, ProjectConfig, ServiceConfig, SiteServiceConfig, TypedServiceConfig,
};
use std::collections::HashMap;

fn load_or_create_config() -> CliResult<(std::path::PathBuf, LocaldConfig)> {
    let cwd = std::env::current_dir()?;
    let config_path = cwd.join("locald.toml");

    let config = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        toml::from_str(&content).context("Failed to parse existing locald.toml")?
    } else {
        let project_name = cwd
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("my-project")
            .to_string();

        LocaldConfig {
            project: ProjectConfig {
                name: project_name.clone(),
                domain: Some(format!("{project_name}.localhost")),
                workspace: None,
                constellation: None,
            },
            plugins: HashMap::new(),
            services: HashMap::new(),
            worktrees: None,
        }
    };

    Ok((config_path, config))
}

fn save_config(path: &std::path::Path, config: &LocaldConfig) -> CliResult<()> {
    let toml_string = toml::to_string_pretty(config)?;
    std::fs::write(path, toml_string)?;
    Ok(())
}

fn start_project() -> CliResult<()> {
    let cwd = std::env::current_dir()?;
    crate::client::stream_boot_events(&locald_core::IpcRequest::Start {
        project_path: cwd,
        inherited_env: crate::handlers::inherited_service_env(),
        verbose: false,
    })?;
    println!("Project started successfully.");
    Ok(())
}

fn postgres_service_config(version: Option<String>) -> ServiceConfig {
    ServiceConfig::Typed(TypedServiceConfig::Postgres(PostgresServiceConfig {
        common: CommonServiceConfig {
            port: None,
            env: HashMap::new(),
            depends_on: Vec::new(),
            health_check: None,
            stop_signal: None,
        },
        version,
    }))
}

fn insert_postgres_service(config: &mut LocaldConfig, name: &str, version: Option<String>) {
    config
        .services
        .insert(name.to_string(), postgres_service_config(version));
}

pub fn add_exec(command: String, name: Option<String>, port: Option<u16>) -> CliResult<()> {
    let (config_path, mut config) = load_or_create_config()?;
    let service_name = name.unwrap_or_else(|| "web".to_string());

    let service_config = ServiceConfig::Typed(TypedServiceConfig::Exec(ExecServiceConfig {
        common: CommonServiceConfig {
            port,
            env: HashMap::new(),
            depends_on: Vec::new(),
            health_check: None,
            stop_signal: None,
        },
        command: Some(command),
        workdir: None,
        build: None,
    }));

    config.services.insert(service_name.clone(), service_config);
    save_config(&config_path, &config)?;

    println!("Updated locald.toml with service '{service_name}'");
    start_project()?;

    Ok(())
}

pub fn add_container(
    image: String,
    name: Option<String>,
    container_port: Option<u16>,
    command: Option<String>,
) -> CliResult<()> {
    let (config_path, mut config) = load_or_create_config()?;
    let service_name = name.unwrap_or_else(|| "redis".to_string());

    let service_config =
        ServiceConfig::Typed(TypedServiceConfig::Container(ContainerServiceConfig {
            common: CommonServiceConfig {
                port: None,
                env: HashMap::new(),
                depends_on: Vec::new(),
                health_check: None,
                stop_signal: None,
            },
            image,
            command,
            container_port,
            workdir: None,
        }));

    config.services.insert(service_name.clone(), service_config);
    save_config(&config_path, &config)?;

    println!("Updated locald.toml with container service '{service_name}'");
    start_project()?;

    Ok(())
}

pub fn add_postgres(name: &str, version: Option<String>) -> CliResult<()> {
    let (config_path, mut config) = load_or_create_config()?;
    insert_postgres_service(&mut config, name, version);
    save_config(&config_path, &config)?;

    println!("Updated locald.toml with postgres service '{name}'");
    start_project()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_config() -> LocaldConfig {
        LocaldConfig {
            project: ProjectConfig {
                name: "pg-test".to_string(),
                domain: Some("pg-test.localhost".to_string()),
                workspace: None,
                constellation: None,
            },
            plugins: HashMap::new(),
            services: HashMap::new(),
            worktrees: None,
        }
    }

    #[test]
    fn insert_postgres_service_generates_typed_postgres_service() {
        let mut config = empty_config();

        insert_postgres_service(&mut config, "db", None);

        let service = config.services.get("db").expect("db service exists");
        match service {
            ServiceConfig::Typed(TypedServiceConfig::Postgres(pg)) => {
                assert_eq!(pg.version, None);
                assert_eq!(pg.common.port, None);
                assert!(pg.common.env.is_empty());
                assert!(pg.common.depends_on.is_empty());
                assert!(pg.common.health_check.is_none());
                assert!(pg.common.stop_signal.is_none());
            }
            other => panic!("expected typed postgres service, got {other:?}"),
        }
    }

    #[test]
    fn insert_postgres_service_preserves_requested_version() {
        let mut config = empty_config();

        insert_postgres_service(&mut config, "database", Some("15".to_string()));

        let service = config
            .services
            .get("database")
            .expect("database service exists");
        match service {
            ServiceConfig::Typed(TypedServiceConfig::Postgres(pg)) => {
                assert_eq!(pg.version.as_deref(), Some("15"));
            }
            other => panic!("expected typed postgres service, got {other:?}"),
        }
    }

    #[test]
    fn postgres_service_serializes_as_typed_postgres_config() {
        let mut config = empty_config();
        insert_postgres_service(&mut config, "db", Some("15".to_string()));

        let toml = toml::to_string_pretty(&config).expect("config serializes");

        assert!(toml.contains("[services.db]"), "{toml}");
        assert!(toml.contains("type = \"postgres\""), "{toml}");
        assert!(toml.contains("version = \"15\""), "{toml}");
    }
}

pub fn add_site(
    path: &std::path::Path,
    name: Option<String>,
    port: Option<u16>,
    build: Option<String>,
) -> CliResult<()> {
    let (config_path, mut config) = load_or_create_config()?;
    let service_name = name.unwrap_or_else(|| "site".to_string());

    let service_config = ServiceConfig::Typed(TypedServiceConfig::Site(SiteServiceConfig {
        common: CommonServiceConfig {
            port,
            env: HashMap::new(),
            depends_on: Vec::new(),
            health_check: None,
            stop_signal: None,
        },
        path: path.to_string_lossy().to_string(),
        build: build.unwrap_or_default(),
        name: String::new(), // Injected at runtime
    }));

    config.services.insert(service_name.clone(), service_config);
    save_config(&config_path, &config)?;

    println!("Updated locald.toml with site service '{service_name}'");
    start_project()?;

    Ok(())
}
