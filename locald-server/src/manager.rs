use anyhow::{Context, Result};
use locald_core::config::LocaldConfig;
use locald_core::ipc::ServiceStatus;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{info, error};

pub struct Service {
    pub config: LocaldConfig,
    pub process: Option<Child>,
    pub port: Option<u16>,
    pub path: PathBuf,
}

#[derive(Clone)]
pub struct ProcessManager {
    services: Arc<Mutex<HashMap<String, Service>>>,
}

impl ProcessManager {
    pub fn new() -> Self {
        Self {
            services: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn start(&self, path: PathBuf) -> Result<()> {
        // Read config
        let config_path = path.join("locald.toml");
        let config_content = tokio::fs::read_to_string(&config_path).await
            .context("Failed to read locald.toml")?;
        let config: LocaldConfig = toml::from_str(&config_content)
            .context("Failed to parse locald.toml")?;

        for (service_name, service_config) in &config.services {
            let name = format!("{}:{}", config.project.name, service_name);
            
            // Check if already running
            let mut services = self.services.lock().await;
            if let Some(service) = services.get_mut(&name) {
                 // Check if actually running
                 if let Some(child) = &mut service.process {
                     if let Ok(None) = child.try_wait() {
                         info!("Service {} is already running", name);
                         continue;
                     }
                 }
            }

            // Find free port
            let port = {
                let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
                listener.local_addr()?.port()
            };

            info!("Starting service {} on port {}", name, port);

            // Spawn process
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(&service_config.command);
            cmd.current_dir(&path);
            cmd.env("PORT", port.to_string());
            for (k, v) in &service_config.env {
                cmd.env(k, v);
            }
            cmd.stdin(Stdio::null());
            cmd.stdout(Stdio::inherit()); 
            cmd.stderr(Stdio::inherit());

            let child = cmd.spawn().context("Failed to spawn process")?;

            services.insert(name, Service {
                config: config.clone(),
                process: Some(child),
                port: Some(port),
                path: path.clone(),
            });
        }

        Ok(())
    }

    pub async fn stop(&self, name: &str) -> Result<()> {
        let mut services = self.services.lock().await;
        if let Some(service) = services.get_mut(name) {
            if let Some(mut child) = service.process.take() {
                info!("Stopping service {}", name);
                child.kill().await?;
                service.port = None;
            }
        }
        Ok(())
    }

    pub async fn list(&self) -> Vec<ServiceStatus> {
        let mut services = self.services.lock().await;
        let mut results = Vec::new();
        
        for (name, service) in services.iter_mut() {
            let mut is_running = false;
            if let Some(child) = &mut service.process {
                 match child.try_wait() {
                     Ok(Some(_)) => {
                         // Exited
                         service.process = None;
                         service.port = None;
                     }
                     Ok(None) => {
                         is_running = true;
                     }
                     Err(e) => {
                         error!("Error checking process status for {}: {}", name, e);
                     }
                 }
            }
            
            results.push(ServiceStatus {
                name: name.clone(),
                pid: service.process.as_ref().and_then(|p| p.id()),
                port: service.port,
                status: if is_running { "running".to_string() } else { "stopped".to_string() },
                url: if is_running {
                    service.port.map(|port| {
                        if let Some(domain) = &service.config.project.domain {
                            format!("http://{}:{}", domain, port)
                        } else {
                            format!("http://localhost:{}", port)
                        }
                    })
                } else {
                    None
                },
            });
        }
        results
    }

    pub async fn shutdown(&self) -> Result<()> {
        let mut services = self.services.lock().await;
        for (name, service) in services.iter_mut() {
            if let Some(mut child) = service.process.take() {
                info!("Stopping service {} (shutdown)", name);
                let _ = child.kill().await;
            }
        }
        Ok(())
    }
}
