use anyhow::{Context, Result};
use postgresql_embedded::{PostgreSQL, Settings};
use semver::VersionReq;
use std::path::PathBuf;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, broadcast};
use tracing::{info, warn};

/// Manages a `PostgreSQL` service instance.
#[derive(Debug)]
pub struct PostgresRunner {
    name: String,
    version: String,
    port: u16,
    data_dir: PathBuf,
    process: Arc<Mutex<Option<tokio::process::Child>>>,
    log_tx: broadcast::Sender<(String, String)>,
}

impl PostgresRunner {
    /// Create a new `PostgresRunner`.
    pub fn new(name: String, version: String, port: u16, data_dir: PathBuf) -> Self {
        let (log_tx, _) = broadcast::channel(1000);
        Self {
            name,
            version,
            port,
            data_dir,
            process: Arc::new(Mutex::new(None)),
            log_tx,
        }
    }

    /// Subscribe to the log stream.
    pub fn subscribe_logs(&self) -> broadcast::Receiver<(String, String)> {
        self.log_tx.subscribe()
    }

    /// Start the `PostgreSQL` service.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The data directory cannot be created.
    /// - Postgres setup fails.
    /// - The postgres binary cannot be found.
    /// - The process fails to spawn.
    pub async fn start(&self) -> Result<()> {
        {
            let process_guard = self.process.lock().await;
            if process_guard.is_some() {
                info!("Postgres service {} is already running", self.name);
                return Ok(());
            }
        }

        info!(
            "Starting Postgres service {} (v{}) on port {} with data dir {:?}",
            self.name, self.version, self.port, self.data_dir
        );

        // Ensure data directory exists (Async)
        if !self.data_dir.exists() {
            tokio::fs::create_dir_all(&self.data_dir)
                .await
                .context("Failed to create data directory")?;
        }

        // Define installation directory
        let install_dir = directories::ProjectDirs::from("com", "locald", "locald").map_or_else(
            || PathBuf::from(".locald/postgres-dist"),
            |d| d.data_dir().join("postgres-dist"),
        );

        let version_req = VersionReq::from_str(&self.version).unwrap_or(VersionReq::STAR);

        let settings = Settings {
            port: self.port,
            version: version_req,
            data_dir: self.data_dir.clone(),
            installation_dir: install_dir.clone(),
            temporary: false,
            ..Default::default()
        };

        // Use postgresql_embedded to install and initdb
        let mut postgres = PostgreSQL::new(settings);
        postgres.setup().await.context("Failed to setup Postgres")?;

        // Find the binary
        let binary_path = self.find_postgres_binary(&install_dir).await?;
        info!("Found postgres binary at {:?}", binary_path);

        // Run postgres manually
        let mut cmd = Command::new(&binary_path);
        cmd.arg("-D").arg(&self.data_dir);
        cmd.arg("-p").arg(self.port.to_string());
        cmd.arg("-h").arg("127.0.0.1"); // Bind to localhost only

        // Capture logs
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Ensure it dies when we die (best effort)
        cmd.kill_on_drop(true);

        let mut child = cmd.spawn().context("Failed to spawn postgres")?;

        let stdout = child.stdout.take().context("Failed to capture stdout")?;
        let stderr = child.stderr.take().context("Failed to capture stderr")?;

        let name = self.name.clone();
        let log_tx = self.log_tx.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                info!("[postgres:{}] {}", name, line);
                if log_tx.send(("stdout".to_string(), line)).is_err() {
                    break;
                }
            }
        });

        let name = self.name.clone();
        let log_tx = self.log_tx.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                info!("[postgres:{}] {}", name, line);
                if log_tx.send(("stderr".to_string(), line)).is_err() {
                    break;
                }
            }
        });

        {
            let mut process_guard = self.process.lock().await;
            *process_guard = Some(child);
        }
        info!("Postgres service {} started successfully", self.name);

        Ok(())
    }

    async fn find_postgres_binary(&self, root: &PathBuf) -> Result<PathBuf> {
        let mut read_dir = tokio::fs::read_dir(root).await?;
        let mut candidates = Vec::new();

        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                let bin = path.join("bin").join("postgres");
                if bin.exists()
                    && let Ok(metadata) = tokio::fs::metadata(&path).await
                    && let Ok(modified) = metadata.modified()
                {
                    candidates.push((bin, modified));
                }
            }
        }

        // Sort by modified time, descending
        candidates.sort_by(|a, b| b.1.cmp(&a.1));

        if let Some((bin, _)) = candidates.into_iter().next() {
            return Ok(bin);
        }

        anyhow::bail!("Could not find postgres binary in {}", root.display());
    }

    /// Stop the `PostgreSQL` service.
    ///
    /// # Errors
    ///
    /// Returns an error if the process cannot be killed.
    pub async fn stop(&self) -> Result<()> {
        let child_opt = {
            let mut process_guard = self.process.lock().await;
            process_guard.take()
        };

        if let Some(mut child) = child_opt {
            info!("Stopping Postgres service {}", self.name);

            if let Some(pid) = child.id() {
                let pid = nix::unistd::Pid::from_raw(pid as i32);
                if let Err(e) = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM) {
                    warn!("Failed to send SIGTERM to postgres: {}", e);
                }
            }

            match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
                Ok(_) => info!("Postgres service {} stopped", self.name),
                Err(_) => {
                    warn!("Postgres service {} did not stop, killing", self.name);
                    if let Err(e) = child.kill().await {
                        warn!("Failed to kill postgres: {}", e);
                    }
                }
            }
        }
        Ok(())
    }

    /// Check if the service is running.
    pub async fn is_running(&self) -> bool {
        let process_guard = self.process.lock().await;
        process_guard.is_some()
    }

    /// Get the port number.
    pub const fn port(&self) -> u16 {
        self.port
    }
}
