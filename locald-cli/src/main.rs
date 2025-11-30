use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use locald_core::{IpcRequest, IpcResponse, LocaldConfig};

mod client;

#[derive(Parser)]
#[command(name = "locald")]
#[command(about = "Local development proxy and process manager", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Ping the locald daemon
    Ping,
    /// Start the locald daemon
    Server,
    /// Start a service in the current directory
    Start {
        /// Path to the service directory (default: current directory)
        #[arg(default_value = ".")]
        path: std::path::PathBuf,
    },
    /// Stop a running service. If no name is provided, stops all services defined in locald.toml in the current directory.
    Stop {
        /// Name of the service to stop
        name: Option<String>,
    },
    /// List running services
    Status,
    /// Shutdown the locald daemon
    Shutdown,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Ping => {
            match client::send_request(IpcRequest::Ping) {
                Ok(response) => println!("Received: {:?}", response),
                Err(e) => println!("Error communicating with locald: {}", e),
            }
        }
        Commands::Server => {
            // Check if already running
            let running = matches!(client::send_request(IpcRequest::Ping), Ok(IpcResponse::Pong));

            if running {
                println!("locald-server is already running.");
            } else {
                let exe_path = std::env::current_exe()?;
                let server_path = exe_path.parent().unwrap().join("locald-server");

                if !server_path.exists() {
                    anyhow::bail!("Could not find locald-server binary at {:?}", server_path);
                }

                println!("Starting locald-server in the background...");
                let log_file = std::fs::File::create("/tmp/locald.log")?;
                
                // Use setsid to detach the server from the current session so it survives Ctrl-C
                std::process::Command::new("setsid")
                    .arg(server_path)
                    .stdout(log_file.try_clone()?)
                    .stderr(log_file)
                    .spawn()?;
                    
                println!("locald-server started. Logs at /tmp/locald.log");
            }
        }
        Commands::Start { path } => {
            let abs_path = std::fs::canonicalize(path)?;
            match client::send_request(IpcRequest::Start { path: abs_path }) {
                Ok(response) => println!("{:?}", response),
                Err(e) => println!("Error: {}", e),
            }
        }
        Commands::Stop { name } => {
            let names = if let Some(n) = name {
                vec![n.clone()]
            } else {
                // Try to read locald.toml in current directory
                let config_path = std::env::current_dir()?.join("locald.toml");
                if !config_path.exists() {
                    anyhow::bail!("No service name provided and no locald.toml found in current directory.");
                }
                let config_content = std::fs::read_to_string(&config_path)
                    .context("Failed to read locald.toml")?;
                let config: LocaldConfig = toml::from_str(&config_content)
                    .context("Failed to parse locald.toml")?;
                
                config.services.keys()
                    .map(|service_name| format!("{}:{}", config.project.name, service_name))
                    .collect()
            };

            for service_name in names {
                match client::send_request(IpcRequest::Stop { name: service_name.clone() }) {
                    Ok(response) => println!("Stopping {}: {:?}", service_name, response),
                    Err(e) => println!("Error stopping {}: {}", service_name, e),
                }
            }
        }
        Commands::Status => {
            match client::send_request(IpcRequest::Status) {
                Ok(IpcResponse::Status(services)) => {
                    if services.is_empty() {
                        println!("No services running.");
                    } else {
                        println!("{:<30} {:<10} {:<10} {:<10}", "NAME", "STATUS", "PID", "PORT");
                        for service in services {
                            println!("{:<30} {:<10} {:<10} {:<10}", 
                                service.name, 
                                service.status, 
                                service.pid.map(|p| p.to_string()).unwrap_or_default(),
                                service.port.map(|p| p.to_string()).unwrap_or_default()
                            );
                        }
                    }
                }
                Ok(response) => println!("Unexpected response: {:?}", response),
                Err(e) => println!("Error: {}", e),
            }
        }
        Commands::Shutdown => {
            match client::send_request(IpcRequest::Shutdown) {
                Ok(response) => println!("{:?}", response),
                Err(e) => println!("Error: {}", e),
            }
        }
    }

    Ok(())
}
