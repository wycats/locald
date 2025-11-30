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
            println!("Starting locald server...");
            println!("Please run `locald-server` directly for now.");
        }
    }

    Ok(())
}
