use crate::runtime::process::ProcessRuntime;
use anyhow::Result;
use async_stream::stream;
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use locald_core::config::{ServiceConfig, TypedServiceConfig};
use locald_core::ipc::{LogEntry, LogStream, ServiceMetrics};
use locald_core::service::{
    RuntimeState, ServiceCommand, ServiceContext, ServiceController, ServiceFactory,
};
use locald_core::state::{HealthStatus, ServiceState};
use nix::sys::signal::Signal;
use portable_pty::{Child, MasterPty};
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tokio::sync::{Mutex, broadcast};
use tracing::warn;

pub struct ExecController {
    id: String,
    runtime: ProcessRuntime,
    config: ServiceConfig,
    project_root: PathBuf,
    // Runtime state
    child: Option<StdMutex<Box<dyn Child + Send>>>,
    pty_master: Option<StdMutex<Box<dyn MasterPty + Send>>>,
    container_id: Option<String>,
    port: Option<u16>,
    log_tx: broadcast::Sender<LogEntry>,
    bundle_dir: Option<PathBuf>,
    env: std::collections::HashMap<String, String>,
    system: StdMutex<System>,
}

impl fmt::Debug for ExecController {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecController")
            .field("id", &self.id)
            .field("config", &self.config)
            .field("project_root", &self.project_root)
            .field("container_id", &self.container_id)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl ExecController {
    #[must_use]
    pub fn new(
        id: String,
        runtime: ProcessRuntime,
        config: ServiceConfig,
        project_root: PathBuf,
        port: Option<u16>,
        env: std::collections::HashMap<String, String>,
    ) -> Self {
        let (log_tx, _) = broadcast::channel(100);
        Self {
            id,
            runtime,
            config,
            project_root,
            child: None,
            pty_master: None,
            container_id: None,
            port,
            log_tx,
            bundle_dir: None,
            env,
            system: StdMutex::new(System::new()),
        }
    }

    fn resolve_env(&self) -> std::collections::HashMap<String, String> {
        self.env.clone()
    }
}

#[async_trait]
impl ServiceController for ExecController {
    fn id(&self) -> &str {
        &self.id
    }

    async fn prepare(&mut self) -> Result<()> {
        match &self.config {
            ServiceConfig::Typed(TypedServiceConfig::Exec(c)) | ServiceConfig::Legacy(c) => {
                if let Some(_build_config) = &c.build {
                    let service_path = c.workdir.as_ref().map_or_else(
                        || self.project_root.clone(),
                        |wd| self.project_root.join(wd),
                    );

                    let env = self.resolve_env();

                    // Setup log callback
                    let log_tx = self.log_tx.clone();
                    let id = self.id.clone();
                    let log_callback = std::sync::Arc::new(move |line: String| {
                        let tx = log_tx.clone();
                        let id = id.clone();
                        tokio::spawn(async move {
                            let timestamp = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();
                            let timestamp = i64::try_from(timestamp).unwrap_or(i64::MAX);

                            let _ = tx.send(LogEntry {
                                timestamp,
                                service: id,
                                stream: LogStream::Stdout,
                                message: line,
                            });
                        });
                    });

                    let bundle_dir = self
                        .runtime
                        .prepare_cnb_container(
                            self.id.clone(),
                            &service_path,
                            c.command.as_ref(),
                            &env,
                            self.port,
                            false, // TODO: Pass verbose flag?
                            Some(log_callback),
                        )
                        .await?;

                    self.bundle_dir = Some(bundle_dir);
                }
            }
            ServiceConfig::Typed(TypedServiceConfig::Container(c)) => {
                let env = self.resolve_env();
                let bundle_dir = self
                    .runtime
                    .prepare_container(
                        self.id.clone(),
                        c.image.clone(),
                        c.command.clone(),
                        &env,
                        self.port,
                        &self.project_root,
                    )
                    .await?;
                self.bundle_dir = Some(bundle_dir);
            }
            ServiceConfig::Typed(
                TypedServiceConfig::Postgres(_)
                | TypedServiceConfig::Worker(_)
                | TypedServiceConfig::Site(_),
            ) => {}
        }

        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (child, master, container_id, mut log_rx) = if let Some(bundle_dir) = &self.bundle_dir {
            self.runtime
                .start_container_process(self.id.clone(), bundle_dir)?
        } else {
            let (command, workdir) = match &self.config {
                ServiceConfig::Typed(TypedServiceConfig::Exec(c)) | ServiceConfig::Legacy(c) => {
                    (c.command.clone(), c.workdir.clone())
                }
                ServiceConfig::Typed(TypedServiceConfig::Worker(c)) => {
                    (Some(c.command.clone()), c.workdir.clone())
                }
                ServiceConfig::Typed(
                    TypedServiceConfig::Container(_)
                    | TypedServiceConfig::Postgres(_)
                    | TypedServiceConfig::Site(_),
                ) => anyhow::bail!("Invalid config for ExecController (Host Process)"),
            };

            let service_path = workdir.map_or_else(
                || self.project_root.clone(),
                |wd| self.project_root.join(wd),
            );

            let env = self.resolve_env();

            let cmd_str = command.ok_or_else(|| anyhow::anyhow!("Command is required"))?;
            self.runtime.start_host_process(
                self.id.clone(),
                &service_path,
                &cmd_str,
                &env,
                self.port,
            )?
        };

        self.child = Some(StdMutex::new(child));
        self.pty_master = Some(StdMutex::new(master));
        self.container_id = Some(container_id);

        // Spawn log forwarder
        let log_tx = self.log_tx.clone();
        let id = self.id.clone();
        tokio::spawn(async move {
            while let Some(entry) = log_rx.recv().await {
                if let Err(_e) = log_tx.send(entry) {
                    tracing::trace!("Failed to broadcast log for {}: no subscribers", id);
                }
            }
        });

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(child_mutex) = self.child.take() {
            if let Ok(mut child) = child_mutex.into_inner() {
                let signal = self.config.common().stop_signal.as_deref().map_or(
                    Signal::SIGTERM,
                    |s| match s.to_uppercase().as_str() {
                        "SIGINT" | "INT" => Signal::SIGINT,
                        "SIGQUIT" | "QUIT" => Signal::SIGQUIT,
                        "SIGKILL" | "KILL" => Signal::SIGKILL,
                        _ => Signal::SIGTERM,
                    },
                );

                crate::runtime::process::ProcessRuntime::terminate_process(
                    &mut child, &self.id, signal,
                )
                .await;
            }
        }

        if let Some(container_id) = &self.container_id {
            if let Err(e) = self.runtime.stop_shim_container(container_id) {
                warn!("Cleanup warning: {:#}", e);
            }
        }

        self.pty_master = None;
        self.container_id = None;

        Ok(())
    }

    async fn read_state(&self) -> RuntimeState {
        let (is_running, pid) = self.child.as_ref().map_or((false, None), |child_mutex| {
            child_mutex.lock().map_or((false, None), |mut child| {
                let running = matches!(child.try_wait(), Ok(None));
                (running, child.process_id())
            })
        });

        RuntimeState {
            pid,
            port: self.port,
            status: if is_running {
                ServiceState::Running
            } else {
                ServiceState::Stopped
            },
            health_status: if is_running {
                HealthStatus::Healthy
            } else {
                HealthStatus::Unknown
            }, // Simplified
        }
    }

    async fn logs(&self) -> BoxStream<'static, LogEntry> {
        let mut rx = self.log_tx.subscribe();
        Box::pin(stream! {
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
        Ok(())
    }

    fn snapshot(&self) -> serde_json::Value {
        serde_json::Value::Null
    }

    async fn restore(&mut self, _state: serde_json::Value) -> Result<()> {
        Ok(())
    }

    async fn metrics(&self) -> Result<Option<ServiceMetrics>> {
        let pid = match self.child.as_ref() {
            Some(child_mutex) => {
                let guard = child_mutex.lock().unwrap();
                guard.process_id()
            }
            None => return Ok(None),
        };

        let Some(pid) = pid else {
            return Ok(None);
        };

        let mut sys = self.system.lock().unwrap();
        let pid = Pid::from_u32(pid);

        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);

        if let Some(process) = sys.process(pid) {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            return Ok(Some(ServiceMetrics {
                name: self.id.clone(),
                cpu_percent: process.cpu_usage(),
                memory_bytes: process.memory(),
                timestamp: i64::try_from(timestamp).unwrap_or(0),
            }));
        }

        Ok(None)
    }
}

#[derive(Debug)]
pub struct ExecFactory {
    runtime: ProcessRuntime,
}

impl ExecFactory {
    #[must_use]
    pub fn new(runtime: ProcessRuntime) -> Self {
        Self { runtime }
    }
}

impl ServiceFactory for ExecFactory {
    fn can_handle(&self, config: &ServiceConfig) -> bool {
        match config {
            ServiceConfig::Typed(TypedServiceConfig::Exec(c)) => c.image.is_none(),
            ServiceConfig::Legacy(c) => c.image.is_none(),
            ServiceConfig::Typed(TypedServiceConfig::Worker(_)) => true,
            ServiceConfig::Typed(TypedServiceConfig::Container(_)) => true,
            ServiceConfig::Typed(TypedServiceConfig::Postgres(_)) => false,
            ServiceConfig::Typed(TypedServiceConfig::Site(_)) => false,
        }
    }

    fn create(
        &self,
        name: String,
        config: &ServiceConfig,
        ctx: &ServiceContext,
    ) -> Arc<Mutex<dyn ServiceController>> {
        Arc::new(Mutex::new(ExecController::new(
            name,
            self.runtime.clone(),
            config.clone(),
            ctx.project_root.clone(),
            ctx.port,
            ctx.env.clone(),
        )))
    }
}
