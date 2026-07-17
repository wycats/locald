use crate::client;
use crate::error::{CliError, CliResult};
use anyhow::Context;
use locald_core::{IpcRequest, IpcResponse, LocaldConfig};
use std::process::Command;

pub fn run_task(service_name: &str, command: &[String]) -> CliResult<()> {
    // Resolve full name if needed
    let full_name = {
        let config_path = std::env::current_dir()?.join("locald.toml");
        if config_path.exists() {
            std::fs::read_to_string(&config_path).map_or_else(
                |_| service_name.to_string(),
                |content| {
                    if let Ok(config) = toml::from_str::<LocaldConfig>(&content) {
                        format!("{}:{}", config.project.name, service_name)
                    } else {
                        service_name.to_string()
                    }
                },
            )
        } else {
            service_name.to_string()
        }
    };

    // Fetch environment
    let env = match client::send_request(&IpcRequest::GetServiceEnv {
        name: full_name.clone(),
    })? {
        IpcResponse::ServiceEnv(env) => env,
        IpcResponse::Error(msg) => {
            // Check if it's a "Service not found" error and hint about `try`
            if msg.contains("not found") {
                eprintln!("Error: Service '{service_name}' not found.");
                eprintln!("Hint: If you meant to run an ad-hoc command, use `locald try` instead.");
                eprintln!(
                    "      Example: `locald try \"{service_name} {}\"`",
                    command.join(" ")
                );
            }
            return Err(CliError::message(format!(
                "Failed to get environment for {full_name}: {msg}"
            )));
        }
        r => return Err(CliError::message(format!("Unexpected response: {r:?}"))),
    };

    // Run command
    let status = Command::new("sh")
        .arg("-c")
        .arg(command.join(" "))
        .envs(env)
        .status()
        .context("Failed to execute command")?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}
