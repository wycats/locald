use anyhow::Result;
use tracing::{info, warn, error};
use crate::manager::ProcessManager;
use crate::proxy::ProxyManager;

mod ipc;
mod manager;
mod proxy;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    info!("locald-server starting...");

    let manager = ProcessManager::new();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel(1);

    // Run IPC server
    let manager_clone = manager.clone();
    tokio::spawn(async move {
        if let Err(e) = ipc::run_ipc_server(manager_clone, shutdown_tx).await {
            warn!("IPC server error: {}", e);
        }
    });

    // Run Proxy server
    let proxy = ProxyManager::new(manager.clone());
    tokio::spawn(async move {
        // Try port 80 first
        if let Err(e) = proxy.start(80).await {
            warn!("Failed to bind port 80: {}. Trying port 8080...", e);
            if let Err(e) = proxy.start(8080).await {
                warn!("Failed to bind port 8080: {}. Trying port 8081...", e);
                if let Err(e) = proxy.start(8081).await {
                    error!("Failed to bind port 8081: {}. Proxy disabled.", e);
                }
            }
        }
    });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => info!("Received Ctrl+C, shutting down"),
        _ = shutdown_rx.recv() => info!("Received shutdown signal"),
    }

    info!("Stopping all services...");
    if let Err(e) = manager.shutdown().await {
        warn!("Error shutting down services: {}", e);
    }

    let _ = std::fs::remove_file("/tmp/locald.sock");

    info!("locald-server stopped");
    Ok(())
}
