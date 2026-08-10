use anyhow::{Context, Result};
use postgresql_embedded::{PostgreSQL, Settings};
use semver::VersionReq;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::str::FromStr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, broadcast};
use tracing::{info, warn};

const POSTGRES_SETUP_COMMAND: &str = "__postgres-setup";
const MAX_POSTGRES_VERSION_LENGTH: usize = 128;

/// Nonsecret settings passed to the isolated Postgres setup helper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostgresSetup {
    version: String,
    port: u16,
    data_dir: PathBuf,
    installation_dir: PathBuf,
}

impl PostgresSetup {
    /// Validate one bounded Postgres setup request.
    ///
    /// # Errors
    ///
    /// Returns an error when the version exceeds the command-line bound or
    /// either filesystem path is not absolute.
    pub fn new(
        version: String,
        port: u16,
        data_dir: PathBuf,
        installation_dir: PathBuf,
    ) -> Result<Self> {
        anyhow::ensure!(
            version.len() <= MAX_POSTGRES_VERSION_LENGTH,
            "Postgres version exceeds {MAX_POSTGRES_VERSION_LENGTH} bytes"
        );
        anyhow::ensure!(
            data_dir.is_absolute(),
            "Postgres data directory must be absolute: {}",
            data_dir.display()
        );
        anyhow::ensure!(
            installation_dir.is_absolute(),
            "Postgres installation directory must be absolute: {}",
            installation_dir.display()
        );

        Ok(Self {
            version,
            port,
            data_dir,
            installation_dir,
        })
    }

    fn command(&self, executable: &Path) -> Result<Command> {
        anyhow::ensure!(
            executable.is_absolute(),
            "Postgres setup executable must be absolute: {}",
            executable.display()
        );

        let mut command = Command::new(executable);
        command
            .arg(POSTGRES_SETUP_COMMAND)
            .arg("--version")
            .arg(&self.version)
            .arg("--port")
            .arg(self.port.to_string())
            .arg("--data-dir")
            .arg(&self.data_dir)
            .arg("--installation-dir")
            .arg(&self.installation_dir)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        Ok(command)
    }
}

/// Perform opaque `postgresql_embedded` setup inside an isolated helper process.
///
/// # Errors
///
/// Returns an error if installation, extraction, or database initialization fails.
pub async fn setup_postgres(request: PostgresSetup) -> Result<()> {
    let version_req = VersionReq::from_str(&request.version).unwrap_or(VersionReq::STAR);
    let settings = Settings {
        port: request.port,
        version: version_req,
        data_dir: request.data_dir,
        installation_dir: request.installation_dir,
        temporary: false,
        ..Default::default()
    };

    let mut postgres = PostgreSQL::new(settings);
    let setup_result = postgres.setup().await.context("Failed to setup Postgres");
    drop(postgres);
    setup_result
}

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

        let data_dir = absolute_path(&self.data_dir)?;

        // Ensure data directory exists (Async)
        if !data_dir.exists() {
            tokio::fs::create_dir_all(&data_dir)
                .await
                .context("Failed to create data directory")?;
        }

        // Define installation directory
        let install_dir = absolute_path(
            &directories::ProjectDirs::from("com", "locald", "locald").map_or_else(
                || PathBuf::from(".locald/postgres-dist"),
                |d| d.data_dir().join("postgres-dist"),
            ),
        )?;

        let setup = PostgresSetup::new(
            self.version.clone(),
            self.port,
            data_dir.clone(),
            install_dir.clone(),
        )?;
        let executable = std::env::current_exe().context("Failed to locate locald executable")?;
        let mut setup_command = setup.command(&executable)?;

        // The helper starts without any daemon-owned publisher descriptor. Its
        // later opaque extraction and initdb children therefore cannot inherit
        // descriptors received by this process after the helper was created.
        let setup_permit = crate::process_spawn::ProcessSpawnBarrier::global().enter_spawn();
        let setup_spawn = setup_command.spawn();
        drop(setup_permit);
        let mut setup_child = setup_spawn.context("Failed to spawn Postgres setup helper")?;
        let setup_status = setup_child
            .wait()
            .await
            .context("Failed to wait for Postgres setup helper")?;
        anyhow::ensure!(
            setup_status.success(),
            "Failed to setup Postgres: helper exited with {setup_status}"
        );

        // Find the binary
        let binary_path = self.find_postgres_binary(&install_dir).await?;
        info!("Found postgres binary at {:?}", binary_path);

        // Run postgres manually
        let mut cmd = Command::new(&binary_path);
        cmd.arg("-D").arg(&data_dir);
        cmd.arg("-p").arg(self.port.to_string());
        cmd.arg("-h").arg("127.0.0.1"); // Bind to localhost only

        // Capture logs
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Ensure it dies when we die (best effort)
        cmd.kill_on_drop(true);

        let spawn_permit = crate::process_spawn::ProcessSpawnBarrier::global().enter_spawn();
        let spawn_result = cmd.spawn();
        drop(spawn_permit);
        let mut child = spawn_result.context("Failed to spawn postgres")?;

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
        candidates.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));

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

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()
        .context("Failed to resolve current directory")?
        .join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_command_contains_only_the_bounded_setup_request() {
        let request = PostgresSetup::new(
            "15.3".to_string(),
            54321,
            PathBuf::from("/data/postgres"),
            PathBuf::from("/data/postgres-dist"),
        )
        .expect("valid setup request");

        let command = request
            .command(Path::new("/opt/locald/bin/locald"))
            .expect("valid helper command");
        let command = command.as_std();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(command.get_program(), "/opt/locald/bin/locald");
        assert_eq!(
            args,
            [
                "__postgres-setup",
                "--version",
                "15.3",
                "--port",
                "54321",
                "--data-dir",
                "/data/postgres",
                "--installation-dir",
                "/data/postgres-dist",
            ]
        );
    }

    #[test]
    fn setup_request_rejects_relative_paths_and_unbounded_versions() {
        let relative_data = PostgresSetup::new(
            "15".to_string(),
            5432,
            PathBuf::from("postgres"),
            PathBuf::from("/data/postgres-dist"),
        )
        .expect_err("relative data path must fail");
        assert!(relative_data.to_string().contains("must be absolute"));

        let long_version = PostgresSetup::new(
            "1".repeat(MAX_POSTGRES_VERSION_LENGTH + 1),
            5432,
            PathBuf::from("/data/postgres"),
            PathBuf::from("/data/postgres-dist"),
        )
        .expect_err("unbounded version must fail");
        assert!(long_version.to_string().contains("exceeds"));
    }
}
