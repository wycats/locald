use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::stream::BoxStream;
use locald_core::config::{ServiceConfig, SiteServiceConfig, TypedServiceConfig};
use locald_core::ipc::{LogEntry, LogStream, ServiceMetrics};
use locald_core::service::{
    RuntimeState, ServiceCommand, ServiceContext, ServiceController, ServiceFactory,
};
use locald_core::state::{HealthStatus, ServiceState};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::{Mutex, broadcast};
use tracing::{error, info, warn};

use crate::static_server;

#[derive(Debug)]
pub struct SiteService {
    name: String,
    config: SiteServiceConfig,
    project_root: PathBuf,
    port: Option<u16>,
    server_handle: Option<tokio::task::JoinHandle<()>>,
    watcher_handle: Option<tokio::task::JoinHandle<()>>,
    log_sender: broadcast::Sender<LogEntry>,
    shared: Arc<Mutex<SharedState>>,
}

#[derive(Debug)]
struct SharedState {
    status: ServiceState,
    health_status: HealthStatus,
}

impl SiteService {
    #[must_use]
    pub fn new(
        name: String,
        config: SiteServiceConfig,
        project_root: PathBuf,
        port: Option<u16>,
    ) -> Self {
        let (tx, _) = broadcast::channel(100);
        Self {
            name,
            config,
            project_root,
            port,
            server_handle: None,
            watcher_handle: None,
            log_sender: tx,
            shared: Arc::new(Mutex::new(SharedState {
                status: ServiceState::Stopped,
                health_status: HealthStatus::Unknown,
            })),
        }
    }

    fn log(&self, message: String, stream: LogStream) {
        Self::log_static(&self.name, &self.log_sender, message, stream);
    }

    fn log_static(
        name: &str,
        sender: &broadcast::Sender<LogEntry>,
        message: String,
        stream: LogStream,
    ) {
        // We prefix the service name with "build:" to distinguish build logs from runtime logs
        // in the unified stream. The frontend can filter on this if needed.
        let entry = LogEntry {
            service: format!("{}:build", name),
            stream,
            message,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        let _ = sender.send(entry);
    }

    async fn run_build_static(
        name: &str,
        config: &SiteServiceConfig,
        project_root: &PathBuf,
        log_sender: &broadcast::Sender<LogEntry>,
    ) -> Result<()> {
        if config.build.is_empty() {
            return Ok(());
        }

        Self::log_static(
            name,
            log_sender,
            format!("Running build: {}", config.build),
            LogStream::Stdout,
        );

        let parts: Vec<&str> = config.build.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(());
        }

        let mut cmd = Command::new(parts[0]);
        cmd.args(&parts[1..]).current_dir(project_root);

        let output = cmd.output().await.context("Failed to run build command")?;

        if !output.stdout.is_empty() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                Self::log_static(name, log_sender, line.to_string(), LogStream::Stdout);
            }
        }

        if !output.stderr.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            for line in stderr.lines() {
                Self::log_static(
                    name,
                    log_sender,
                    format!("ERR: {}", line),
                    LogStream::Stderr,
                );
            }
        }

        if output.status.success() {
            Self::log_static(
                name,
                log_sender,
                "Build succeeded".to_string(),
                LogStream::Stdout,
            );
        } else {
            Self::log_static(
                name,
                log_sender,
                "Build failed".to_string(),
                LogStream::Stderr,
            );
            warn!("Build failed for {}", name);
        }

        Ok(())
    }

    fn start_watcher(&mut self) -> Result<()> {
        use notify::{RecursiveMode, Watcher};
        use std::time::Duration;

        let (tx, mut rx) = tokio::sync::mpsc::channel(100);
        let mut watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                let _ = tx.blocking_send(res);
            })?;

        watcher.watch(&self.project_root, RecursiveMode::Recursive)?;

        self.log(
            format!("Watching {}", self.project_root.display()),
            LogStream::Stdout,
        );

        let name = self.name.clone();
        let config = self.config.clone();
        let project_root = self.project_root.clone();
        let log_sender = self.log_sender.clone();

        let handle = tokio::spawn(async move {
            let _watcher = watcher; // Keep watcher alive
            let mut debounce_timer: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
            let debounce_duration = Duration::from_millis(500);

            loop {
                tokio::select! {
                    Some(res) = rx.recv() => {
                        match res {
                            Ok(event) => {
                                if event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove() {
                                    // Ignore .git, target, node_modules
                                    let ignore = event.paths.iter().any(|p| {
                                        let s = p.to_string_lossy();
                                        s.contains("/.git/") || s.contains("/target/") || s.contains("/node_modules/")
                                    });

                                    if !ignore {
                                        // Reset timer
                                        debounce_timer = Some(Box::pin(tokio::time::sleep(debounce_duration)));
                                    }
                                }
                            }
                            Err(e) => {
                                error!("Watch error: {}", e);
                            }
                        }
                    }
                    _ = async {
                        if let Some(timer) = debounce_timer.as_mut() {
                            timer.await;
                            true
                        } else {
                            std::future::pending().await
                        }
                    } => {
                        debounce_timer = None;
                        let _ = Self::run_build_static(&name, &config, &project_root, &log_sender).await;
                    }
                }
            }
        });

        self.watcher_handle = Some(handle);
        Ok(())
    }
}

#[async_trait]
impl ServiceController for SiteService {
    fn id(&self) -> &str {
        &self.name
    }

    #[allow(clippy::unused_async)]
    async fn prepare(&mut self) -> Result<()> {
        let mut state = self.shared.lock().await;
        state.health_status = HealthStatus::Starting;
        state.status = ServiceState::Building;
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        info!("Starting site service: {}", self.name);

        // 1. Start the static server
        let path = self.project_root.join(&self.config.path);
        let port = self.port.unwrap_or(8080);
        let log_sender = self.log_sender.clone();

        let server_handle = tokio::spawn(async move {
            if let Err(e) = static_server::run_static_server(port, path, log_sender).await {
                error!("Static server failed: {}", e);
            }
        });

        self.server_handle = Some(server_handle);

        // 2. Start watcher
        self.start_watcher()?;

        // 3. Trigger initial build in background
        let name = self.name.clone();
        let config = self.config.clone();
        let project_root = self.project_root.clone();
        let log_sender = self.log_sender.clone();
        let shared = self.shared.clone();

        tokio::spawn(async move {
            let result = Self::run_build_static(&name, &config, &project_root, &log_sender).await;

            let mut state = shared.lock().await;
            if result.is_ok() {
                state.status = ServiceState::Running;
                state.health_status = HealthStatus::Healthy;
            } else {
                state.health_status = HealthStatus::Unhealthy;
                // We keep it "Building" or "Running" so logs can be seen?
                // If we set it to Stopped, it might disappear or show as stopped.
                // Let's set it to Running but Unhealthy so user can see logs.
                state.status = ServiceState::Running;
            }
        });

        self.log(
            format!("Serving {} on port {}", self.config.path, port),
            LogStream::Stdout,
        );

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        info!("Stopping site service: {}", self.name);

        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }

        if let Some(handle) = self.watcher_handle.take() {
            handle.abort();
        }

        let mut state = self.shared.lock().await;
        state.status = ServiceState::Stopped;
        state.health_status = HealthStatus::Unknown;
        Ok(())
    }

    async fn read_state(&self) -> RuntimeState {
        let state = self.shared.lock().await;
        RuntimeState {
            pid: None, // No single PID for this service
            port: self.port,
            status: state.status,
            health_status: state.health_status,
        }
    }

    async fn logs(&self) -> BoxStream<'static, LogEntry> {
        let mut rx = self.log_sender.subscribe();
        Box::pin(async_stream::stream! {
            while let Ok(entry) = rx.recv().await {
                yield entry;
            }
        })
    }

    fn get_metadata(&self, key: &str) -> Option<String> {
        match key {
            "port" => self.port.map(|p| p.to_string()),
            _ => None,
        }
    }

    async fn execute_command(&mut self, _cmd: ServiceCommand) -> Result<()> {
        // TODO: Implement rebuild command
        Ok(())
    }

    fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({})
    }

    #[allow(clippy::unused_async)]
    async fn restore(&mut self, _state: serde_json::Value) -> Result<()> {
        Ok(())
    }

    #[allow(clippy::unused_async)]
    async fn metrics(&self) -> Result<Option<ServiceMetrics>> {
        Ok(None)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SiteFactory;

impl ServiceFactory for SiteFactory {
    fn can_handle(&self, config: &ServiceConfig) -> bool {
        matches!(config, ServiceConfig::Typed(TypedServiceConfig::Site(_)))
    }

    #[allow(clippy::panic)]
    fn create(
        &self,
        name: String,
        config: &ServiceConfig,
        ctx: &ServiceContext,
    ) -> Arc<Mutex<dyn ServiceController>> {
        let site_config = match config {
            ServiceConfig::Typed(TypedServiceConfig::Site(c)) => c.clone(),
            ServiceConfig::Typed(_) | ServiceConfig::Legacy(_) => {
                panic!("Invalid config type for SiteFactory")
            }
        };

        Arc::new(Mutex::new(SiteService::new(
            name,
            site_config,
            ctx.project_root.clone(),
            ctx.port,
        )))
    }
}
