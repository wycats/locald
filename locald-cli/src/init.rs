use anyhow::{Context, Result};
use dialoguer::{Input, Confirm};
use locald_core::config::{LocaldConfig, ProjectConfig, ServiceConfig};
use std::collections::HashMap;


pub fn run() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let config_path = cwd.join("locald.toml");

    if config_path.exists() {
        println!("locald.toml already exists in this directory.");
        if !Confirm::new().with_prompt("Do you want to overwrite it?").interact()? {
            return Ok(());
        }
    }

    println!("Initializing new locald project...");

    let default_name = cwd.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("my-project");

    let project_name: String = Input::new()
        .with_prompt("Project Name")
        .default(default_name.to_string())
        .interact_text()?;

    let domain: String = Input::new()
        .with_prompt("Local Domain (optional)")
        .default(format!("{}.local", project_name))
        .allow_empty(true)
        .interact_text()?;

    let domain = if domain.trim().is_empty() { None } else { Some(domain) };

    let mut services = HashMap::new();

    loop {
        println!("\n--- Add a Service ---");
        let service_name: String = Input::new()
            .with_prompt("Service Name (e.g., web, api)")
            .default("web".to_string())
            .interact_text()?;

        let command: String = Input::new()
            .with_prompt("Command to run")
            .interact_text()?;

        let port_str: String = Input::new()
            .with_prompt("Port (optional, usually handled by $PORT)")
            .allow_empty(true)
            .interact_text()?;

        let port = if port_str.trim().is_empty() {
            None
        } else {
            Some(port_str.parse::<u16>().context("Invalid port number")?)
        };

        let workdir: String = Input::new()
            .with_prompt("Working Directory (optional)")
            .allow_empty(true)
            .interact_text()?;
        
        let workdir = if workdir.trim().is_empty() { None } else { Some(workdir) };

        let service_config = ServiceConfig {
            command,
            workdir,
            env: HashMap::new(),
            port,
            depends_on: Vec::new(),
        };

        services.insert(service_name, service_config);

        if !Confirm::new().with_prompt("Add another service?").default(false).interact()? {
            break;
        }
    }

    let config = LocaldConfig {
        project: ProjectConfig {
            name: project_name,
            domain,
        },
        services,
    };

    let toml_string = toml::to_string_pretty(&config)?;
    std::fs::write(&config_path, toml_string)?;

    println!("\nSuccessfully created locald.toml!");
    println!("Run `locald start` to launch your project.");

    Ok(())
}
