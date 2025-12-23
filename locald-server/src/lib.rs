//! # locald-server
//!
//! `locald-server` is the core daemon for the `locald` system. It manages the lifecycle of services,
//! handles IPC requests from the CLI, and proxies HTTP/HTTPS traffic.
//!
//! ## Lifecycle
//!
//! 1.  **Startup**: The `run` function initializes the `ProcessManager`, `ProxyManager`, and `NotifyServer`.
//! 2.  **Restoration**: It attempts to restore the state of previously running services.
//! 3.  **Event Loop**: It listens for IPC requests and process exit notifications.
//! 4.  **Shutdown**: It gracefully stops all services and cleans up resources.
//!
//! ## Entry Points
//!
//! *   **Main Loop**: [`run`](crate::run)
//! *   **Configuration**: [`config_loader::ConfigLoader`]
//!
//! ## Warning
//!
//! This crate is primarily intended for internal use by the `locald` binary.
//! The API is not guaranteed to be stable.

// =========================================================================
//  Strict Lints: Safety, Hygiene, and Documentation
// =========================================================================

// 1. Logic & Safety
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/wycats/dotlocal/phase-23-advanced-service-config/locald-docs/public/favicon.svg"
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/wycats/dotlocal/phase-23-advanced-service-config/locald-docs/public/favicon.svg"
)]
#![allow(clippy::let_underscore_must_use)] // Don't swallow errors with `let _`
#![warn(clippy::await_holding_lock)] // Prevent Async Deadlocks (Critical)
#![allow(clippy::manual_let_else)] // Enforces clean "Guard Clause" style
#![allow(clippy::unwrap_used)] // Force error propagation (no panics)
#![allow(clippy::expect_used)] // Force error propagation
#![warn(clippy::wildcard_enum_match_arm)] // Force explicit enum matching
#![warn(clippy::redundant_pattern_matching)] // Catch redundant matches
#![warn(unreachable_pub)] // Warn if an item is pub but not reachable from crate root
#![warn(clippy::match_wildcard_for_single_variants)] // Catch `_ =>` when only one variant remains
#![warn(clippy::must_use_candidate)] // Suggest `#[must_use]` for pure functions
#![warn(clippy::unused_async)] // Catch async functions that don't await

// 2. Numeric Safety (Critical for PIDs/Ports)
#![warn(clippy::cast_possible_truncation)] // Warn on u64 -> u32 (potential data loss)
#![allow(clippy::cast_possible_wrap)] // Warn on u32 -> i32 (potential overflow)

// 3. Observability
#![allow(clippy::print_stdout)] // Ban println! (Use tracing::info!)
#![allow(clippy::print_stderr)] // Ban eprintln! (Use tracing::error!)

// 4. Import Hygiene
#![warn(clippy::wildcard_imports)] // Ban `use crate::*` (Explicit imports only)
#![allow(clippy::shadow_unrelated)] // Ban accidental variable shadowing

// 5. Documentation
#![allow(missing_docs)] // TODO: Enable later
#![allow(clippy::missing_errors_doc)] // TODO: Enable later

// 6. Other
#![allow(deprecated)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::significant_drop_tightening)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::unnecessary_debug_formatting)]
#![allow(clippy::uninlined_format_args)]

#[doc(hidden)]
pub mod api;
#[doc(hidden)]
pub mod assets;
#[doc(hidden)]
// pub mod cert; // Moved to locald-utils
pub mod config_loader;
#[doc(hidden)]
pub mod container;
#[doc(hidden)]
pub mod health;
#[doc(hidden)]
pub mod ipc;
#[doc(hidden)]
pub mod logging;
#[doc(hidden)]
pub mod manager;
#[doc(hidden)]
pub mod plugins;
#[doc(hidden)]
// pub mod notify; // Moved to locald-utils
#[doc(hidden)]
pub mod proxy;
#[doc(hidden)]
pub mod runtime;
#[doc(hidden)]
pub mod service;
#[doc(hidden)]
pub mod shim_client;
#[doc(hidden)]
pub mod state;
#[doc(hidden)]
pub mod static_server;
#[doc(hidden)]
#[doc(hidden)]
pub mod toolbar;

#[cfg(test)]
mod proxy_test;
#[cfg(test)]
mod test_create;

use crate::manager::ProcessManager;
use crate::proxy::ProxyManager;
use anyhow::{Context, Result};
use daemonize::Daemonize;
use nix::unistd::execv;
use std::ffi::CString;
use std::fs::File;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use tracing::{error, info, warn};
use tracing_subscriber::prelude::*;

#[derive(Debug, Clone, Copy)]
pub enum ShutdownReason {
    Stop,
    Restart,
}

#[allow(clippy::disallowed_methods)]
pub fn run(foreground: bool, version: String) -> Result<()> {
    // Idempotency check: if already running, exit successfully
    if is_already_running() {
        println!("locald is already running.");
        return Ok(());
    }

    if !foreground {
        let stdout = File::create("/tmp/locald.out")?;
        let stderr = File::create("/tmp/locald.err")?;

        let daemonize = Daemonize::new()
            .pid_file("/tmp/locald.pid")
            .chown_pid_file(true)
            .working_directory("/tmp")
            .stdout(stdout)
            .stderr(stderr);

        match daemonize.start() {
            Ok(()) => println!("locald-server started in background"),
            Err(e) => {
                eprintln!("Error starting daemon: {e}");
                return Err(e.into());
            }
        }
    }

    // Initialize logging
    let (log_tx, _) = tokio::sync::broadcast::channel(100);

    let broadcast_layer = logging::BroadcastLayer {
        sender: log_tx.clone(),
    };
    let fmt_layer = tracing_subscriber::fmt::layer();

    let _ = tracing_subscriber::registry()
        .with(fmt_layer)
        .with(broadcast_layer)
        .try_init();

    // Install default crypto provider
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Start Tokio runtime
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(version, log_tx))
}

fn is_already_running() -> bool {
    // Try to connect to the socket to see if a server is listening
    locald_utils::ipc::socket_path().is_ok_and(|path| UnixStream::connect(path).is_ok())
}

async fn async_main(
    version: String,
    log_tx: tokio::sync::broadcast::Sender<locald_core::ipc::LogEntry>,
) -> Result<()> {
    info!("locald-server starting... (version: {})", version);

    // Load configuration
    let config = crate::config_loader::ConfigLoader::load()
        .await
        .map(|loader| loader.global)
        .unwrap_or_else(|e| {
            warn!("Failed to load global config: {e}. Using defaults.");
            locald_core::config::GlobalConfig::default()
        });

    // The notify socket must be sandbox-aware (tests and parallel sandboxes), otherwise
    // multiple daemon instances will contend for the same fixed path.
    let notify_path = locald_utils::ipc::socket_path()
        .map(|p| p.with_file_name("locald-notify.sock"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/locald-notify.sock"));

    // Initialize dependencies
    let docker = match bollard::Docker::connect_with_local_defaults() {
        Ok(d) => Some(std::sync::Arc::new(d)),
        Err(e) => {
            warn!("Failed to connect to Docker: {e}. Docker-based services will be unavailable.");
            None
        }
    };

    let state_manager = std::sync::Arc::new(
        crate::state::StateManager::new().context("Failed to initialize state manager")?,
    );

    let registry = std::sync::Arc::new(tokio::sync::Mutex::new(
        locald_core::registry::Registry::load()
            .await
            .unwrap_or_default(),
    ));

    let manager = ProcessManager::new(
        notify_path.clone(),
        docker,
        state_manager,
        registry,
        Some(log_tx),
    )?;
    manager.spawn_metrics_collector();

    // Initialize ContainerManager
    let data_dir = directories::ProjectDirs::from("com", "locald", "locald")
        .map(|dirs| dirs.data_local_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".locald"));
    let container_manager = std::sync::Arc::new(crate::container::ContainerManager::new(&data_dir));

    // Notify Server
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::channel(100);
    // We need to handle potential failure binding the socket
    let notify_server = match locald_utils::notify::NotifyServer::new(notify_path).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to bind notify socket: {e}");
            return Err(e);
        }
    };

    tokio::spawn(async move {
        notify_server.run(notify_tx).await;
    });

    let manager_clone = manager.clone();
    tokio::spawn(async move {
        while let Some((pid, _msg)) = notify_rx.recv().await {
            manager_clone.handle_notify(pid).await;
        }
    });

    // Restore state
    let manager_restore = manager.clone();
    tokio::spawn(async move {
        if let Err(e) = manager_restore.restore().await {
            warn!("Failed to restore state: {e}");
        }
    });

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<ShutdownReason>(1);

    // Run IPC server
    let manager_clone = manager.clone();
    let container_manager_clone = container_manager.clone();
    let version_clone = version.clone();
    let shutdown_tx_ipc = shutdown_tx.clone();
    let ipc_handle = tokio::spawn(async move {
        ipc::run_ipc_server(
            manager_clone,
            container_manager_clone,
            shutdown_tx_ipc,
            version_clone,
        )
        .await
    });

    // Spawn upgrade watcher
    let container_manager_clone = container_manager.clone();
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        watch_for_upgrade(container_manager_clone, shutdown_tx_clone).await;
    });

    // Initialize CertManager
    let cert_manager = match locald_utils::cert::CertManager::new().await {
        Ok(cm) => Some(std::sync::Arc::new(cm)),
        Err(e) => {
            warn!("Failed to initialize CertManager: {e}. HTTPS will be disabled.");
            None
        }
    };

    // Run Proxy server
    let api_router = crate::api::router(manager.clone());
    let proxy = std::sync::Arc::new(ProxyManager::new(
        std::sync::Arc::new(manager.clone()),
        api_router,
        cert_manager,
    ));

    // Bind HTTP
    let listener_http = if let Ok(port_str) = std::env::var("LOCALD_HTTP_PORT") {
        let port = port_str.parse::<u16>().unwrap_or(8080);
        info!("Binding HTTP to configured port: {}", port);
        match proxy.bind_http(port).await {
            Ok(l) => Some(l),
            Err(e) => {
                error!("Failed to bind configured port {}: {}", port, e);
                None
            }
        }
    } else if config.server.privileged_ports {
        match proxy.bind_http(80).await {
            Ok(l) => Some(l),
            Err(e) => {
                if !config.server.fallback_ports {
                    error!(
                        "Failed to bind port 80: {e}. You may need to run `sudo locald admin setup` or configure `privileged_ports = false`."
                    );
                    // We return error here to stop startup if privileged ports are required but failed.
                    return Err(e);
                }
                warn!("Failed to bind port 80: {e}. Trying fallback...");
                None
            }
        }
    } else {
        None
    };

    let listener_http = if let Some(l) = listener_http {
        Some(l)
    } else if config.server.fallback_ports {
        match proxy.bind_http(8080).await {
            Ok(l) => Some(l),
            Err(e) => {
                warn!("Failed to bind port 8080: {e}. Trying 8081...");
                match proxy.bind_http(8081).await {
                    Ok(l) => Some(l),
                    Err(e) => {
                        error!("Failed to bind port 8081: {e}. Proxy disabled.");
                        None
                    }
                }
            }
        }
    } else {
        None
    };

    if let Some(l) = listener_http {
        let proxy_clone = proxy.clone();
        tokio::spawn(async move {
            if let Err(e) = proxy_clone.serve_http(l).await {
                error!("HTTP proxy server error: {e}");
            }
        });
    }

    // Bind HTTPS
    let mut listener_https: Option<std::net::TcpListener> = None;
    if let Ok(port_str) = std::env::var("LOCALD_HTTPS_PORT") {
        let port = port_str.parse::<u16>().unwrap_or(8443);
        info!("Binding HTTPS to configured port: {}", port);
        match proxy.bind_https(port).await {
            Ok(l) => listener_https = Some(l),
            Err(e) => error!("Failed to bind configured port {}: {}", port, e),
        }
    } else if config.server.privileged_ports {
        match proxy.bind_https(443).await {
            Ok(l) => listener_https = Some(l),
            Err(e) => {
                if !config.server.fallback_ports {
                    error!(
                        "Failed to bind port 443: {e}. You may need to run `sudo locald admin setup` or configure `privileged_ports = false`."
                    );
                    return Err(e);
                }
                warn!("Failed to bind port 443: {e}. Trying fallback...");
            }
        }
    }

    if listener_https.is_none() && config.server.fallback_ports {
        match proxy.bind_https(8443).await {
            Ok(l) => listener_https = Some(l),
            Err(e) => error!("Failed to bind port 8443: {e}. HTTPS disabled."),
        }
    }

    if let Some(l) = listener_https {
        let proxy_clone = proxy.clone();
        tokio::spawn(async move {
            if let Err(e) = proxy_clone.serve_https(l).await {
                error!("HTTPS proxy server error: {e}");
            }
        });
    }

    let reason = tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, shutting down");
            ShutdownReason::Stop
        },
        r = shutdown_rx.recv() => {
            info!("Received shutdown signal");
            r.unwrap_or(ShutdownReason::Stop)
        },
        result = ipc_handle => {
            match result {
                Ok(Err(e)) => error!("IPC server failed: {e}"),
                Ok(Ok(())) => info!("IPC server exited normally"),
                Err(e) => error!("IPC server task panicked: {e}"),
            }
            ShutdownReason::Stop
        }
    };

    info!("Stopping all services...");
    if let Err(e) = manager.shutdown().await {
        warn!("Error shutting down services: {e}");
    }

    if let Ok(path) = locald_utils::ipc::socket_path() {
        let _ = tokio::fs::remove_file(path).await;
    }
    let _ = tokio::fs::remove_file("/tmp/locald.pid").await;

    if matches!(reason, ShutdownReason::Restart) {
        info!("Restarting process...");
        let exe_path = std::env::current_exe()?;
        let exe = CString::new(exe_path.as_os_str().as_bytes())
            .context("Executable path contained an interior NUL")?;

        let mut argv = Vec::new();
        argv.push(exe.clone());
        for arg in std::env::args().skip(1) {
            argv.push(CString::new(arg).context("Argument contained an interior NUL")?);
        }

        let err = execv(&exe, &argv)
            .err()
            .context("execv unexpectedly returned Ok")?;
        error!("Failed to exec: {}", err);
        return Err(err.into());
    }

    info!("locald-server stopped");
    Ok(())
}

async fn watch_for_upgrade(
    container_manager: std::sync::Arc<crate::container::ContainerManager>,
    shutdown_tx: tokio::sync::mpsc::Sender<ShutdownReason>,
) {
    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to get current exe path: {}", e);
            return;
        }
    };

    let initial_mtime = match std::fs::metadata(&exe_path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(e) => {
            error!("Failed to get exe metadata: {}", e);
            return;
        }
    };

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));

    loop {
        interval.tick().await;

        let current_mtime = match std::fs::metadata(&exe_path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };

        if current_mtime != initial_mtime {
            info!("Detected binary upgrade.");

            let active = container_manager.active_count();
            if active == 0 {
                info!("No active ephemeral tasks. Initiating restart...");
                let _ = shutdown_tx.send(ShutdownReason::Restart).await;
                break;
            }
            info!("Deferring restart: {} active ephemeral tasks.", active);
        }
    }
}
