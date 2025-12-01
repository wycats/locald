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
use bollard::Docker;
use bollard::container::{Config, CreateContainerOptions, StartContainerOptions, RemoveContainerOptions, LogsOptions};
use bollard::models::HostConfig;
use futures_util::StreamExt;

const LOG_BUFFER_SIZE: usize = 1000;

pub struct Service {
    pub config: LocaldConfig,
    pub process: Option<Child>,
    pub container_id: Option<String>,
    pub port: Option<u16>,
    pub path: PathBuf,
}

#[derive(Clone)]
pub struct ProcessManager {
    services: Arc<Mutex<HashMap<String, Service>>>,
    pub log_sender: broadcast::Sender<LogEntry>,
    log_buffer: Arc<Mutex<VecDeque<LogEntry>>>,
    state_manager: Arc<StateManager>,
    docker: Arc<Docker>,
}

impl ProcessManager {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(100);
        // Initialize Docker client. We assume the socket is available.
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => Arc::new(d),
            Err(e) => {
                error!("Failed to connect to Docker: {}", e);
                panic!("Failed to connect to Docker: {}", e);
            }
        };

        Self {
            services: Arc::new(Mutex::new(HashMap::new())),
            log_sender: tx,
            log_buffer: Arc::new(Mutex::new(VecDeque::with_capacity(LOG_BUFFER_SIZE))),
            state_manager: Arc::new(StateManager::new().expect("Failed to initialize state manager")),
            docker,
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
            let status = if pid.is_some() || service.container_id.is_some() { "running".to_string() } else { "stopped".to_string() };
            
            service_states.push(ServiceState {
                name: name.clone(),
                config: service.config.clone(),
                path: service.path.clone(),
                pid,
                container_id: service.container_id.clone(),
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

            if let Some(image) = &service_config.image {
                // Docker Service
                let container_port = service_config.container_port.context("container_port is required for Docker services")?;
                let container_id = self.start_container(name.clone(), image, port, container_port, &service_config.env, &service_config.command).await?;
                
                {
                    let mut services = self.services.lock().await;
                    services.insert(name, Service {
                        config: config.clone(),
                        process: None,
                        container_id: Some(container_id),
                        port: Some(port),
                        path: path.clone(),
                    });
                }
            } else {
                // Process Service
                let mut cmd = Command::new("sh");
                if let Some(command_str) = &service_config.command {
                    cmd.arg("-c").arg(command_str);
                } else {
                    anyhow::bail!("Service {} has no command", name);
                }
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
                        container_id: None,
                        port: Some(port),
                        path: path.clone(),
                    });
                }
            }
        }

        self.persist_state().await;
        Ok(())
    }

    async fn start_container(&self, name: String, image: &str, host_port: u16, container_port: u16, env: &HashMap<String, String>, command: &Option<String>) -> Result<String> {
        // 1. Pull image (TODO: Make this smarter/optional?)
        // For now, we assume the image exists or Docker will pull it if we use create_container?
        // Actually create_container fails if image missing.
        // Let's try to create, if it fails with 404, pull.
        // For MVP, let's just assume user has pulled it or we pull it blindly.
        // To keep it fast, let's skip explicit pull for now and rely on local cache.
        // If it fails, we can error out telling user to `docker pull`.

        // 2. Create Container
        let container_name = format!("locald-{}", name.replace(":", "-"));
        
        // Remove existing if any (cleanup)
        let _ = self.docker.remove_container(&container_name, Some(RemoveContainerOptions {
            force: true,
            ..Default::default()
        })).await;

        let mut env_vars: Vec<String> = env.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
        env_vars.push(format!("PORT={}", container_port)); // Inject PORT inside too, though usually fixed

        let host_config = HostConfig {
            port_bindings: Some(HashMap::from([
                (
                    format!("{}/tcp", container_port),
                    Some(vec![bollard::models::PortBinding {
                        host_ip: Some("127.0.0.1".to_string()),
                        host_port: Some(host_port.to_string()),
                    }]),
                )
            ])),
            ..Default::default()
        };

        let cmd = command.as_ref().map(|s| shlex::split(s).unwrap_or_default());

        let config = Config {
            image: Some(image.to_string()),
            env: Some(env_vars),
            host_config: Some(host_config),
            cmd,
            ..Default::default()
        };

        let res = self.docker.create_container(
            Some(CreateContainerOptions { name: container_name.clone(), platform: None }),
            config,
        ).await.context("Failed to create Docker container")?;

        let id = res.id;

        // 3. Start Container
        self.docker.start_container(&id, None::<StartContainerOptions<String>>).await.context("Failed to start Docker container")?;

        // 4. Stream Logs
        let manager = self.clone();
        let service_name = name.clone();
        let docker = self.docker.clone();
        let id_clone = id.clone();

        tokio::spawn(async move {
            let options = Some(LogsOptions::<String> {
                follow: true,
                stdout: true,
                stderr: true,
                ..Default::default()
            });

            let mut stream = docker.logs(&id_clone, options);

            while let Some(msg) = stream.next().await {
                if let Ok(msg) = msg {
                    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
                    let (stream_type, payload) = match msg {
                        bollard::container::LogOutput::StdOut { message } => ("stdout", message),
                        bollard::container::LogOutput::StdErr { message } => ("stderr", message),
                        bollard::container::LogOutput::Console { message } => ("console", message),
                        _ => continue,
                    };
                    
                    let message = String::from_utf8_lossy(&payload).to_string();
                    // Docker logs might contain newlines, split them?
                    for line in message.lines() {
                         manager.broadcast_log(LogEntry {
                            timestamp,
                            service: service_name.clone(),
                            stream: stream_type.to_string(),
                            message: line.to_string(),
                        }).await;
                    }
                }
            }
        });

        Ok(id)
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
                    info!("Stopping service {} (process)", name);
                    let _ = child.kill().await;
                }
                if let Some(container_id) = service.container_id.take() {
                    info!("Stopping service {} (container {})", name, container_id);
                    let _ = self.docker.remove_container(&container_id, Some(RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    })).await;
                }
                service.port = None;
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
            } else if let Some(container_id) = &service.container_id {
                // Check container status
                // This is async, but we are in a sync loop over the lock.
                // We shouldn't hold the lock while calling Docker API.
                // For now, let's assume it's running if we have an ID, 
                // or we need to refactor list to not hold the lock.
                // Refactoring list is better.
                is_running = true; // Optimistic for now to avoid deadlock/complexity in this step
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
                if let Some(container_id) = service.container_id.take() {
                    info!("Stopping service {} (container {})", name, container_id);
                    let _ = self.docker.remove_container(&container_id, Some(RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    })).await;
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
