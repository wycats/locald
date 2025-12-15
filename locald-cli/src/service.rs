use anyhow::{Context, Result};
use locald_core::config::{
    CommonServiceConfig, ContainerServiceConfig, ExecServiceConfig, LocaldConfig,
    PostgresServiceConfig, ProjectConfig, ServiceConfig, SiteServiceConfig, TypedServiceConfig,
};
use std::collections::HashMap;

fn load_or_create_config() -> Result<(std::path::PathBuf, LocaldConfig)> {
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
            },
            services: HashMap::new(),
        }
    };

    Ok((config_path, config))
}

fn save_config(path: &std::path::Path, config: &LocaldConfig) -> Result<()> {
    let toml_string = toml::to_string_pretty(config)?;
    std::fs::write(path, toml_string)?;
    Ok(())
}

fn start_project() -> Result<()> {
    let cwd = std::env::current_dir()?;
    crate::client::stream_boot_events(&locald_core::IpcRequest::Start {
        project_path: cwd,
        verbose: false,
    })?;
    println!("Project started successfully.");
    Ok(())
}

pub fn add_exec(command: String, name: Option<String>, port: Option<u16>) -> Result<()> {
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
        image: None,
        container_port: None,
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
) -> Result<()> {
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

pub fn add_postgres(name: &str, version: Option<String>) -> Result<()> {
    let (config_path, mut config) = load_or_create_config()?;

    let service_config =
        ServiceConfig::Typed(TypedServiceConfig::Postgres(PostgresServiceConfig {
            common: CommonServiceConfig {
                port: None,
                env: HashMap::new(),
                depends_on: Vec::new(),
                health_check: None,
                stop_signal: None,
            },
            version,
        }));

    config.services.insert(name.to_string(), service_config);
    save_config(&config_path, &config)?;

    println!("Updated locald.toml with postgres service '{name}'");
    start_project()?;

    Ok(())
}

pub fn add_site(
    path: &std::path::Path,
    name: Option<String>,
    port: Option<u16>,
    build: Option<String>,
) -> Result<()> {
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
