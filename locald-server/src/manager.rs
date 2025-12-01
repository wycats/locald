use anyhow::{Context, Result};
use locald_core::config::LocaldConfig;
use locald_core::ipc::{ServiceStatus, LogEntry};
use locald_core::state::{ServerState, ServiceState};
use crate::state::StateManager;
use std::collections::{HashMap, VecDeque, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, broadcast};
use tokio::io::{BufReader, AsyncBufReadExt};
use tracing::{info, error};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;

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
    state_manager: Arc<StateManager>,
}

impl ProcessManager {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(100);
        Self {
            services: Arc::new(Mutex::new(HashMap::new())),
            log_sender: tx,
            log_buffer: Arc::new(Mutex::new(VecDeque::with_capacity(LOG_BUFFER_SIZE))),
            state_manager: Arc::new(StateManager::new().expect("Failed to initialize state manager")),
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

    async fn persist_state(&self) {
        let services = self.services.lock().await;
        let mut service_states = Vec::new();
        
        for (name, service) in services.iter() {
            let pid = service.process.as_ref().and_then(|p| p.id());
            let status = if pid.is_some() { "running".to_string() } else { "stopped".to_string() };
            
            service_states.push(ServiceState {
                name: name.clone(),
                config: service.config.clone(),
                path: service.path.clone(),
                pid,
                port: service.port,
                status,
            });
        }
        
        let state = ServerState {
            services: service_states,
        };
        
        if let Err(e) = self.state_manager.save(&state).await {
            error!("Failed to persist state: {}", e);
        }
    }

    pub async fn restore(&self) -> Result<()> {
        let state = match self.state_manager.load().await {
            Ok(s) => s,
            Err(_) => return Ok(()), // No state to restore
        };

        info!("Restoring state: found {} services", state.services.len());

        // Cleanup old processes
        for service_state in &state.services {
            if let Some(pid) = service_state.pid {
                // Try to kill it. 
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
            }
        }

        // Restart projects
        let mut paths = HashSet::new();
        for service_state in state.services {
            if service_state.status == "running" {
                paths.insert(service_state.path);
            }
        }

        for path in paths {
            info!("Restoring project at {:?}", path);
            if let Err(e) = self.start(path.clone()).await {
                error!("Failed to restore project at {:?}: {}", path, e);
            }
        }
        
        Ok(())
    }

    pub async fn start(&self, path: PathBuf) -> Result<()> {
        // Read config
        let config_path = path.join("locald.toml");
        let config_content = tokio::fs::read_to_string(&config_path).await
            .context("Failed to read locald.toml")?;
        let config: LocaldConfig = toml::from_str(&config_content)
            .context("Failed to parse locald.toml")?;

        let sorted_services = Self::resolve_startup_order(&config)?;

        for service_name in sorted_services {
            let service_config = &config.services[&service_name];
            let name = format!("{}:{}", config.project.name, service_name);
            
            // Check if already running
            {
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
            } // Drop lock before spawning

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

            {
                let mut services = self.services.lock().await;
                services.insert(name, Service {
                    config: config.clone(),
                    process: Some(child),
                    port: Some(port),
                    path: path.clone(),
                });
            }
        }

        self.persist_state().await;
        Ok(())
    }

    fn resolve_startup_order(config: &LocaldConfig) -> Result<Vec<String>> {
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        // Initialize
        for name in config.services.keys() {
            in_degree.insert(name.clone(), 0);
            dependents.insert(name.clone(), Vec::new());
        }

        // Build graph
        for (name, service) in &config.services {
            for dep in &service.depends_on {
                if !config.services.contains_key(dep) {
                    anyhow::bail!("Service '{}' depends on unknown service '{}'", name, dep);
                }
                
                // dep -> name
                dependents.get_mut(dep).unwrap().push(name.clone());
                *in_degree.get_mut(name).unwrap() += 1;
            }
        }

        // Find initial nodes (0 dependencies)
        for (name, &degree) in &in_degree {
            if degree == 0 {
                queue.push_back(name.clone());
            }
        }

        let mut sorted = Vec::new();
        while let Some(node) = queue.pop_front() {
            sorted.push(node.clone());

            if let Some(neighbors) = dependents.get(&node) {
                for neighbor in neighbors {
                    let degree = in_degree.get_mut(neighbor).unwrap();
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(neighbor.clone());
                    }
                }
            }
        }

        if sorted.len() != config.services.len() {
            anyhow::bail!("Circular dependency detected in services");
        }

        Ok(sorted)
    }

    pub async fn stop(&self, name: &str) -> Result<()> {
        {
            let mut services = self.services.lock().await;
            if let Some(service) = services.get_mut(name) {
                if let Some(mut child) = service.process.take() {
                    info!("Stopping service {}", name);
                    let _ = child.kill().await;
                    service.port = None;
                }
            }
        }
        self.persist_state().await;
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
        {
            let mut services = self.services.lock().await;
            for (name, service) in services.iter_mut() {
                if let Some(mut child) = service.process.take() {
                    info!("Stopping service {} (shutdown)", name);
                    let _ = child.kill().await;
                }
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
#[cfg(test)]
mod tests {
    use super::*;
    use locald_core::config::{ServiceConfig, ProjectConfig};

    #[test]
    fn test_resolve_startup_order() {
        let mut services = HashMap::new();
        
        services.insert("db".to_string(), ServiceConfig {
            command: "echo db".to_string(),
            port: None,
            env: HashMap::new(),
            workdir: None,
            depends_on: vec![],
        });
        
        services.insert("backend".to_string(), ServiceConfig {
            command: "echo backend".to_string(),
            port: None,
            env: HashMap::new(),
            workdir: None,
            depends_on: vec!["db".to_string()],
        });
        
        services.insert("frontend".to_string(), ServiceConfig {
            command: "echo frontend".to_string(),
            port: None,
            env: HashMap::new(),
            workdir: None,
            depends_on: vec!["backend".to_string()],
        });

        let config = LocaldConfig {
            project: ProjectConfig { name: "test".to_string(), domain: None },
            services,
        };

        let order = ProcessManager::resolve_startup_order(&config).unwrap();
        
        // db must be before backend
        let db_idx = order.iter().position(|s| s == "db").unwrap();
        let backend_idx = order.iter().position(|s| s == "backend").unwrap();
        let frontend_idx = order.iter().position(|s| s == "frontend").unwrap();

        assert!(db_idx < backend_idx);
        assert!(backend_idx < frontend_idx);
    }

    #[test]
    fn test_circular_dependency() {
        let mut services = HashMap::new();
        
        services.insert("a".to_string(), ServiceConfig {
            command: "echo a".to_string(),
            port: None,
            env: HashMap::new(),
            workdir: None,
            depends_on: vec!["b".to_string()],
        });
        
        services.insert("b".to_string(), ServiceConfig {
            command: "echo b".to_string(),
            port: None,
            env: HashMap::new(),
            workdir: None,
            depends_on: vec!["a".to_string()],
        });

        let config = LocaldConfig {
            project: ProjectConfig { name: "test".to_string(), domain: None },
            services,
        };

        let result = ProcessManager::resolve_startup_order(&config);
        assert!(result.is_err());
    }
}
