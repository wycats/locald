use anyhow::{Context, Result};
use locald_core::config::{LocaldConfig, ProjectConfig, ServiceConfig};
use std::collections::HashMap;

pub fn run(command: String, name: Option<String>, port: Option<u16>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let config_path = cwd.join("locald.toml");
    let service_name = name.unwrap_or_else(|| "web".to_string());

    let mut config = if config_path.exists() {
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
                domain: Some(format!("{project_name}.local")),
            },
            services: HashMap::new(),
        }
    };

    let service_config = ServiceConfig {
        command: Some(command),
        workdir: None,
        env: HashMap::new(),
        port,
        depends_on: Vec::new(),
        image: None,
        container_port: None,
        health_check: None,
    };

    config.services.insert(service_name.clone(), service_config);

    let toml_string = toml::to_string_pretty(&config)?;
    std::fs::write(&config_path, toml_string)?;

    println!("Updated locald.toml with service '{service_name}'");

    // Now start the project
    crate::client::send_request(&locald_core::IpcRequest::Start { path: cwd })?;
    println!("Project started successfully.");

    Ok(())
}
