use anyhow::Result;
use async_stream::stream;
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use locald_core::ipc::{LogEntry, LogStream, ServiceMetrics};
use locald_core::service::{RuntimeState, ServiceCommand, ServiceController};
use locald_core::state::{HealthStatus, ServiceState};
use locald_utils::postgres::PostgresRunner;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct PostgresController {
    id: String,
    runner: Arc<PostgresRunner>,
}

impl PostgresController {
    #[must_use]
    pub fn new(id: String, runner: Arc<PostgresRunner>) -> Self {
        Self { id, runner }
    }
}

#[async_trait]
impl ServiceController for PostgresController {
    fn id(&self) -> &str {
        &self.id
    }

    async fn prepare(&mut self) -> Result<()> {
        // PostgresRunner does setup in start(), but we could move it here if we wanted.
        // For now, we'll just say it's ready.
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        self.runner.start().await
    }

    async fn stop(&mut self) -> Result<()> {
        self.runner.stop().await
    }

    async fn read_state(&self) -> RuntimeState {
        let running = self.runner.is_running().await;
        RuntimeState {
            pid: None, // PostgresRunner doesn't expose PID yet
            port: Some(self.runner.port()),
            status: if running {
                ServiceState::Running
            } else {
                ServiceState::Stopped
            },
            health_status: if running {
                HealthStatus::Healthy
            } else {
                HealthStatus::Unknown
            },
        }
    }

    async fn logs(&self) -> BoxStream<'static, LogEntry> {
        let mut rx = self.runner.subscribe_logs();
        let service_name = self.id.clone();

        Box::pin(stream! {
            while let Ok((stream_name, line)) = rx.recv().await {
                yield LogEntry {
                    timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64,
                    service: service_name.clone(),
                    instance_id: None,
                    service_name: None,
                    stream: if stream_name == "stderr" { LogStream::Stderr } else { LogStream::Stdout },
                    message: line,
                };
            }
        })
    }

    fn get_metadata(&self, key: &str) -> Option<String> {
        match key {
            "port" => Some(self.runner.port().to_string()),
            "url" | "connection_string" => Some(format!(
                "postgres://postgres@localhost:{}/postgres",
                self.runner.port()
            )),
            _ => None,
        }
    }

    async fn execute_command(&mut self, _cmd: ServiceCommand) -> Result<()> {
        // Not implemented yet
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

use locald_core::config::{ServiceConfig, TypedServiceConfig};
use locald_core::service::{ServiceContext, ServiceFactory, ServiceKey};
use std::path::PathBuf;
use tokio::sync::Mutex;

#[derive(Debug, Copy, Clone)]
pub struct PostgresFactory;

fn data_root() -> PathBuf {
    directories::ProjectDirs::from("com", "locald", "locald").map_or_else(
        || PathBuf::from(".locald"),
        |directories| directories.data_dir().to_owned(),
    )
}

fn data_dir_for_root(root: &std::path::Path, key: &ServiceKey) -> PathBuf {
    root.join("instances")
        .join(key.instance().to_string())
        .join("postgres")
        .join(key.resource_id())
}

pub(crate) fn data_dir_for(key: &ServiceKey) -> PathBuf {
    data_dir_for_root(&data_root(), key)
}

pub(crate) fn legacy_data_dir(display_name: &str) -> PathBuf {
    data_root().join("postgres").join(display_name)
}

impl ServiceFactory for PostgresFactory {
    fn can_handle(&self, config: &ServiceConfig) -> bool {
        matches!(
            config,
            ServiceConfig::Typed(TypedServiceConfig::Postgres(_))
        )
    }

    fn create(
        &self,
        name: String,
        config: &ServiceConfig,
        ctx: &ServiceContext,
    ) -> Arc<Mutex<dyn ServiceController>> {
        if let ServiceConfig::Typed(TypedServiceConfig::Postgres(pg_config)) = config {
            let data_dir = data_dir_for(&ctx.key);

            // Use port from: 1) config, 2) context (allocated by manager), 3) fallback to 5432
            let port = pg_config
                .common
                .port
                .or_else(|| ctx.bindings.primary_port())
                .unwrap_or(5432);

            let runner = Arc::new(PostgresRunner::new(
                name.clone(),
                pg_config
                    .version
                    .clone()
                    .unwrap_or_else(|| "15".to_string()),
                port,
                data_dir,
            ));

            Arc::new(Mutex::new(PostgresController::new(name, runner)))
        } else {
            // This should be unreachable because can_handle checks it, but we return a dummy or error controller?
            // Since create doesn't return Result, we have to panic or return a broken controller.
            // Ideally create should return Result. For now, we'll log error and panic, or just panic.
            // But clippy complains about panic.
            tracing::error!("PostgresFactory called with invalid config: {:?}", config);
            #[allow(clippy::panic)]
            {
                panic!("PostgresFactory called with invalid config");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::data_dir_for_root;
    use locald_core::identity::ProjectInstanceId;
    use locald_core::service::ServiceKey;
    use tempfile::tempdir;

    fn instance_id(value: &str) -> ProjectInstanceId {
        value.parse().expect("valid project instance ID")
    }

    #[test]
    fn same_name_postgres_services_use_distinct_instance_directories() {
        let root = tempdir().expect("create data root");
        let first = ServiceKey::new(instance_id("00000000-0000-4000-8000-000000000001"), "db");
        let second = ServiceKey::new(instance_id("00000000-0000-4000-8000-000000000002"), "db");

        let first_dir = data_dir_for_root(root.path(), &first);
        let second_dir = data_dir_for_root(root.path(), &second);

        assert_ne!(first_dir, second_dir);
        assert!(
            first_dir.starts_with(
                root.path()
                    .join("instances")
                    .join(first.instance().to_string())
            )
        );
        assert!(
            second_dir.starts_with(
                root.path()
                    .join("instances")
                    .join(second.instance().to_string())
            )
        );
    }
}
