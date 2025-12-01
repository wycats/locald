use anyhow::Result;
use clap::Parser;
use daemonize::Daemonize;
use std::fs::File;
use std::os::unix::net::UnixStream;
use tracing::{info, warn, error};
use crate::manager::ProcessManager;
use crate::proxy::ProxyManager;

mod ipc;
mod manager;
mod proxy;
mod state;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Run in the foreground (do not daemonize)
    #[arg(short, long)]
    foreground: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Idempotency check: if already running, exit successfully
    if is_already_running() {
        println!("locald is already running.");
        return Ok(());
    }

    if !args.foreground {
        let stdout = File::create("/tmp/locald.out").unwrap();
        let stderr = File::create("/tmp/locald.err").unwrap();

        let daemonize = Daemonize::new()
            .pid_file("/tmp/locald.pid")
            .chown_pid_file(true)
            .working_directory("/tmp")
            .stdout(stdout)
            .stderr(stderr);

        match daemonize.start() {
            Ok(_) => println!("locald-server started in background"),
            Err(e) => {
                eprintln!("Error starting daemon: {}", e);
                return Err(e.into());
            }
        }
    }

    // Initialize logging
    tracing_subscriber::fmt::init();

    // Start Tokio runtime
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main())
}

fn is_already_running() -> bool {
    // Try to connect to the socket to see if a server is listening
    UnixStream::connect("/tmp/locald.sock").is_ok()
}

async fn async_main() -> Result<()> {
    info!("locald-server starting...");

    let manager = ProcessManager::new();

    // Restore state
    if let Err(e) = manager.restore().await {
        warn!("Failed to restore state: {}", e);
    }

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
    let _ = std::fs::remove_file("/tmp/locald.pid");

    info!("locald-server stopped");
    Ok(())
}
