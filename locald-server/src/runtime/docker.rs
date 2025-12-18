use anyhow::{Context, Result};
use bollard::Docker;
use bollard::models::ContainerCreateBody;
use bollard::models::HostConfig;
use bollard::query_parameters::{InspectContainerOptions, LogsOptions, StartContainerOptions};
use futures_util::StreamExt;
use locald_core::ipc::{LogEntry, LogStream};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::info;

#[derive(Clone, Debug)]
#[deprecated(
    since = "0.1.3",
    note = "Use ProcessRuntime (locald-shim bundle run --bundle <PATH> --id <ID>) instead"
)]
pub struct DockerRuntime {
    client: Option<Arc<Docker>>,
}

impl DockerRuntime {
    #[must_use]
    pub fn new(client: Option<Arc<Docker>>) -> Self {
        Self { client }
    }

    pub async fn stop_container(&self, id: &str) -> Result<()> {
        let Some(client) = &self.client else {
            return Ok(());
        };

        info!("Stopping Docker container {}", id);
        client
            .remove_container(
                id,
                Some(bollard::container::RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .context("Failed to remove container")
    }

    pub async fn start(
        &self,
        name: String,
        image: &str,
        port: u16,
        container_port: u16,
        env: &HashMap<String, String>,
        command: Option<&String>,
    ) -> Result<(String, bool, mpsc::Receiver<LogEntry>)> {
        let Some(client) = &self.client else {
            anyhow::bail!("Docker is not available");
        };

        // 1. Create Container
        let container_name = format!("locald-{}", name.replace(':', "-"));

        // Remove existing if any (cleanup)
        let _ = client
            .remove_container(
                &container_name,
                Some(bollard::container::RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;

        let mut env_vars: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        env_vars.push(format!("PORT={container_port}"));

        let host_config = HostConfig {
            port_bindings: Some(HashMap::from([(
                format!("{container_port}/tcp"),
                Some(vec![bollard::models::PortBinding {
                    host_ip: Some("127.0.0.1".to_string()),
                    host_port: Some(port.to_string()),
                }]),
            )])),
            ..Default::default()
        };

        let cmd = command.map(|s| shlex::split(s).unwrap_or_default());

        let config = ContainerCreateBody {
            image: Some(image.to_string()),
            env: Some(env_vars),
            host_config: Some(host_config),
            cmd,
            ..Default::default()
        };

        let res = client
            .create_container(
                Some(bollard::container::CreateContainerOptions {
                    name: container_name,
                    ..Default::default()
                }),
                config,
            )
            .await
            .context("Failed to create Docker container")?;

        let id = res.id;

        // Check if image has healthcheck
        let inspect = client
            .inspect_container(&id, None::<InspectContainerOptions>)
            .await?;
        let has_healthcheck = inspect.config.and_then(|c| c.healthcheck).is_some();

        // 2. Start Container
        client
            .start_container(&id, None::<StartContainerOptions>)
            .await
            .context("Failed to start Docker container")?;

        // 3. Stream Logs
        let (tx, rx) = mpsc::channel(100);
        let docker = client.clone();
        let id_clone = id.clone();
        let service_name = name.clone();

        tokio::spawn(async move {
            let options = Some(LogsOptions {
                follow: true,
                stdout: true,
                stderr: true,
                ..Default::default()
            });

            let mut stream = docker.logs(&id_clone, options);

            while let Some(msg) = stream.next().await {
                if let Ok(msg) = msg {
                    let timestamp = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;

                    let entry = LogEntry {
                        timestamp,
                        service: service_name.clone(),
                        stream: LogStream::Stdout, // Docker logs mix? Bollard output has stream type
                        message: msg.to_string().trim().to_string(),
                    };
                    if tx.send(entry).await.is_err() {
                        break;
                    }
                }
            }
        });

        Ok((id, has_healthcheck, rx))
    }
}
