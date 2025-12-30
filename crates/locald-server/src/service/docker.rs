use crate::runtime::docker::DockerRuntime;
use anyhow::Result;
use async_stream::stream;
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use locald_core::config::{ServiceConfig, TypedServiceConfig};
use locald_core::ipc::{LogEntry, ServiceMetrics};
use locald_core::service::{
    RuntimeState, ServiceCommand, ServiceContext, ServiceController, ServiceFactory,
};
use locald_core::state::{HealthStatus, ServiceState};
use std::fmt;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};
use tracing::warn;

pub struct DockerController {
    id: String,
    runtime: DockerRuntime,
    config: ServiceConfig,
    // Runtime state
    container_id: Option<String>,
    port: Option<u16>,
    log_tx: broadcast::Sender<LogEntry>,
    has_healthcheck: bool,
    env: std::collections::HashMap<String, String>,
}

impl fmt::Debug for DockerController {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DockerController")
            .field("id", &self.id)
            .field("config", &self.config)
            .field("container_id", &self.container_id)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl DockerController {
    #[must_use]
    pub fn new(
        id: String,
        runtime: DockerRuntime,
        config: ServiceConfig,
        port: Option<u16>,
        env: std::collections::HashMap<String, String>,
    ) -> Self {
        let (log_tx, _) = broadcast::channel(100);
        Self {
            id,
            runtime,
            config,
            container_id: None,
            port,
            log_tx,
            has_healthcheck: false,
            env,
        }
    }

    fn resolve_env(&self) -> std::collections::HashMap<String, String> {
        self.env.clone()
    }
}

#[async_trait]
impl ServiceController for DockerController {
    fn id(&self) -> &str {
        &self.id
    }

    async fn prepare(&mut self) -> Result<()> {
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (image, command, container_port) = match &self.config {
            ServiceConfig::Typed(TypedServiceConfig::Exec(c)) | ServiceConfig::Legacy(c) => {
                (c.image.clone(), c.command.clone(), c.container_port)
            }
            ServiceConfig::Typed(
                TypedServiceConfig::Worker(_)
                | TypedServiceConfig::Container(_)
                | TypedServiceConfig::Postgres(_)
                | TypedServiceConfig::Site(_),
            ) => {
                anyhow::bail!("Invalid config for DockerController")
            }
        };

        let image = image.ok_or_else(|| anyhow::anyhow!("Image is required"))?;
        let container_port =
            container_port.ok_or_else(|| anyhow::anyhow!("Container port is required"))?;
        let port = self
            .port
            .ok_or_else(|| anyhow::anyhow!("Port is required"))?;
        let env = self.resolve_env();

        let (container_id, has_healthcheck, mut log_rx) = self
            .runtime
            .start(
                self.id.clone(),
                &image,
                port,
                container_port,
                &env,
                command.as_ref(),
            )
            .await?;

        self.container_id = Some(container_id);
        self.has_healthcheck = has_healthcheck;

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
        if let Some(container_id) = &self.container_id {
            if let Err(e) = self.runtime.stop_container(container_id).await {
                warn!("Cleanup warning: {:#}", e);
            }
        }
        self.container_id = None;
        Ok(())
    }

    async fn read_state(&self) -> RuntimeState {
        let is_running = self.container_id.is_some();
        RuntimeState {
            pid: None,
            port: self.port,
            status: if is_running {
                ServiceState::Running
            } else {
                ServiceState::Stopped
            },
            health_status: if is_running {
                if self.has_healthcheck {
                    HealthStatus::Starting // Docker healthcheck will update this eventually? 
                // Actually, HealthMonitor handles updates.
                } else {
                    HealthStatus::Healthy
                }
            } else {
                HealthStatus::Unknown
            },
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
            "container_id" => self.container_id.clone(),
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
        Ok(None)
    }
}

#[derive(Debug)]
pub struct DockerFactory {
    runtime: DockerRuntime,
}

impl DockerFactory {
    #[must_use]
    pub fn new(runtime: DockerRuntime) -> Self {
        Self { runtime }
    }
}

impl ServiceFactory for DockerFactory {
    fn can_handle(&self, config: &ServiceConfig) -> bool {
        match config {
            ServiceConfig::Typed(TypedServiceConfig::Exec(c)) => c.image.is_some(),
            ServiceConfig::Legacy(c) => c.image.is_some(),
            ServiceConfig::Typed(
                TypedServiceConfig::Worker(_)
                | TypedServiceConfig::Container(_)
                | TypedServiceConfig::Postgres(_)
                | TypedServiceConfig::Site(_),
            ) => false,
        }
    }

    fn create(
        &self,
        name: String,
        config: &ServiceConfig,
        ctx: &ServiceContext,
    ) -> Arc<Mutex<dyn ServiceController>> {
        Arc::new(Mutex::new(DockerController::new(
            name,
            self.runtime.clone(),
            config.clone(),
            ctx.port,
            ctx.env.clone(),
        )))
    }
}
