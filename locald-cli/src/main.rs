use anyhow::Result;
use clap::{Parser, Subcommand};
use locald_core::IpcRequest;

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
    /// Stop a running service
    Stop {
        /// Name of the service to stop
        name: String,
    },
    /// List running services
    Status,
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
            let exe_path = std::env::current_exe()?;
            let server_path = exe_path.parent().unwrap().join("locald-server");

            if !server_path.exists() {
                anyhow::bail!("Could not find locald-server binary at {:?}", server_path);
            }

            println!("Starting locald-server in the background...");
            let log_file = std::fs::File::create("/tmp/locald.log")?;
            
            std::process::Command::new(server_path)
                .stdout(log_file.try_clone()?)
                .stderr(log_file)
                .spawn()?;
                
            println!("locald-server started. Logs at /tmp/locald.log");
        }
        Commands::Start { path } => {
            let abs_path = std::fs::canonicalize(path)?;
            match client::send_request(IpcRequest::Start { path: abs_path }) {
                Ok(response) => println!("{:?}", response),
                Err(e) => println!("Error: {}", e),
            }
        }
        Commands::Stop { name } => {
            match client::send_request(IpcRequest::Stop { name: name.clone() }) {
                Ok(response) => println!("{:?}", response),
                Err(e) => println!("Error: {}", e),
            }
        }
        Commands::Status => {
            match client::send_request(IpcRequest::Status) {
                Ok(response) => println!("{:?}", response),
                Err(e) => println!("Error: {}", e),
            }
        }
    }

    Ok(())
}
