use anyhow::{Context, Result};
use locald_core::config::LocaldConfig;
use locald_core::ipc::{ServiceStatus, LogEntry};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, broadcast};
use tokio::io::{BufReader, AsyncBufReadExt};
use tracing::{info, error};

const LOG_BUFFER_SIZE: usize = 1000;

pub struct Service {
    pub config: LocaldConfig,
    pub process: Option<Child>,
    pub port: Option<u16>,
    pub path: PathBuf,
}

#[derive(Clone)]
pub struct ProcessManager {
    services: Arc<Mutex<HashMap<String, Service>>>,
    pub log_sender: broadcast::Sender<LogEntry>,
    log_buffer: Arc<Mutex<VecDeque<LogEntry>>>,
}

impl ProcessManager {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(100);
        Self {
            services: Arc::new(Mutex::new(HashMap::new())),
            log_sender: tx,
            log_buffer: Arc::new(Mutex::new(VecDeque::with_capacity(LOG_BUFFER_SIZE))),
        }
    }

    async fn broadcast_log(&self, entry: LogEntry) {
        // Add to buffer
        let mut buffer = self.log_buffer.lock().await;
        if buffer.len() >= LOG_BUFFER_SIZE {
            buffer.pop_front();
        }
        buffer.push_back(entry.clone());

        // Broadcast (ignore error if no receivers)
        let _ = self.log_sender.send(entry);
    }

    pub async fn get_recent_logs(&self) -> Vec<LogEntry> {
        let buffer = self.log_buffer.lock().await;
        buffer.iter().cloned().collect()
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
            cmd.stdout(Stdio::piped()); 
            cmd.stderr(Stdio::piped());

            let mut child = cmd.spawn().context("Failed to spawn process")?;

            // Spawn log readers
            let stdout = child.stdout.take().expect("Failed to capture stdout");
            let stderr = child.stderr.take().expect("Failed to capture stderr");
            
            let manager = self.clone();
            let service_name_clone = name.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
                    manager.broadcast_log(LogEntry {
                        timestamp,
                        service: service_name_clone.clone(),
                        stream: "stdout".to_string(),
                        message: line,
                    }).await;
                }
            });

            let manager = self.clone();
            let service_name_clone = name.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
                    manager.broadcast_log(LogEntry {
                        timestamp,
                        service: service_name_clone.clone(),
                        stream: "stderr".to_string(),
                        message: line,
                    }).await;
                }
            });

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
                domain: service.config.project.domain.clone(),
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

    pub async fn find_port_by_domain(&self, domain: &str) -> Option<u16> {
        let services = self.services.lock().await;
        for service in services.values() {
            if let Some(d) = &service.config.project.domain {
                if d == domain {
                    // TODO: Handle multiple services with same domain (e.g. pick 'web')
                    return service.port;
                }
            }
        }
        None
    }
}
