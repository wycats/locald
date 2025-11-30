use anyhow::Result;
use tracing::{info, warn};
use crate::manager::ProcessManager;

mod ipc;
mod manager;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    info!("locald-server starting...");

    let manager = ProcessManager::new();

    // Run IPC server
    tokio::spawn(async move {
        if let Err(e) = ipc::run_ipc_server(manager).await {
            warn!("IPC server error: {}", e);
        }
    });

    match tokio::signal::ctrl_c().await {
        Ok(()) => info!("Received Ctrl+C, shutting down"),
        Err(err) => warn!("Unable to listen for shutdown signal: {}", err),
    }

    let _ = std::fs::remove_file("/tmp/locald.sock");

    info!("locald-server stopped");
    Ok(())
}
