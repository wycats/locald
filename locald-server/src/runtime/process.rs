use anyhow::{Context, Result};
use locald_builder::{
    BuilderImage, BundleSource, ContainerImage, Lifecycle, LocalLayoutBundleSource, ShimRuntime,
};
use locald_core::ipc::{LogEntry, LogStream};
use locald_oci::{oci_layout, runtime_spec};
use nix::sys::signal::Signal;
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};

type ProcessHandle = (
    Box<dyn Child + Send>,
    Box<dyn MasterPty + Send>,
    String,
    mpsc::Receiver<LogEntry>,
    broadcast::Sender<Vec<u8>>,
);

#[derive(Clone, Debug)]
pub struct ProcessRuntime {
    notify_socket_path: PathBuf,
}

impl ProcessRuntime {
    #[must_use]
    pub fn new(notify_socket_path: PathBuf) -> Self {
        Self { notify_socket_path }
    }

    pub fn kill_pid(&self, pid: i32, signal: Signal) -> Result<()> {
        locald_utils::process::kill_pid(pid, signal)
    }

    pub fn stop_shim_container(&self, id: &str) -> Result<()> {
        // Option A: The shim runs the container as its foreground child.
        // Stopping is handled by terminating the shim process; container state
        // is cleaned up by the shim on exit.
        info!("Stopping Shim container {}", id);
        Ok(())
    }

    fn create_pty() -> Result<portable_pty::PtyPair> {
        let pty_system = NativePtySystem::default();
        pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to create PTY")
    }

    fn spawn_log_streamer(
        reader: Box<dyn std::io::Read + Send>,
        service_name: String,
    ) -> (mpsc::Receiver<LogEntry>, broadcast::Sender<Vec<u8>>) {
        let (tx, rx) = mpsc::channel(100);
        let (pty_tx, _) = broadcast::channel(100);
        let pty_tx_clone = pty_tx.clone();

        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buffer = Vec::new();
            let mut buf = [0u8; 4096];

            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = buf[0..n].to_vec();
                        let _ = pty_tx_clone.send(data.clone());

                        buffer.extend_from_slice(&data);
                        while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                            let line_bytes: Vec<u8> = buffer.drain(0..=pos).collect();
                            let line_len = line_bytes.len();
                            let line_content = if line_len > 0 && line_bytes[line_len - 1] == b'\n' {
                                &line_bytes[..line_len - 1]
                            } else {
                                &line_bytes[..]
                            };
                            
                            let line = String::from_utf8_lossy(line_content).to_string();

                            let timestamp = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();
                            let timestamp = i64::try_from(timestamp).unwrap_or(i64::MAX);

                            let entry = LogEntry {
                                timestamp,
                                service: service_name.clone(),
                                stream: LogStream::Stdout,
                                message: line,
                            };
                            if tx.blocking_send(entry).is_err() {
                                return;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        (rx, pty_tx)
    }

    fn spawn_bundle_process(name: String, bundle_dir: &Path) -> Result<ProcessHandle> {
        let container_id = format!("locald-{}", uuid::Uuid::new_v4());
        let shim_path = ShimRuntime::find_shim()?;

        info!("Spawning shim for {} (container {})", name, container_id);

        let pair = Self::create_pty()?;

        let mut cmd = CommandBuilder::new(shim_path);
        cmd.arg("bundle");
        cmd.arg("run");
        cmd.arg("--bundle");
        cmd.arg(bundle_dir);
        cmd.arg("--id");
        cmd.arg(&container_id);

        let child = pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn process")?;

        let reader = pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;

        let (rx, pty_tx) = Self::spawn_log_streamer(reader, name);
        let master = pair.master;

        Ok((child, master, container_id, rx, pty_tx))
    }

    pub fn start_host_process(
        &self,
        name: String,
        path: &Path,
        command: &str,
        env: &HashMap<String, String>,
        port: Option<u16>,
    ) -> Result<ProcessHandle> {
        info!("Starting host process for service {}", name);

        let pair = Self::create_pty()?;

        // Use sh -c to allow shell expansion and features
        let mut cmd = CommandBuilder::new("sh");
        cmd.arg("-c");
        cmd.arg(command);
        cmd.cwd(path);

        // Set environment variables
        for (k, v) in env {
            cmd.env(k, v);
        }
        if let Some(p) = port {
            cmd.env("PORT", p.to_string());
        }
        cmd.env(
            "NOTIFY_SOCKET",
            self.notify_socket_path.display().to_string(),
        );

        let child = pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn process")?;

        let reader = pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;

        let (rx, pty_tx) = Self::spawn_log_streamer(reader, name);
        let master = pair.master;

        // Generate a pseudo-container ID for tracking
        let container_id = format!("host-{}", uuid::Uuid::new_v4());

        Ok((child, master, container_id, rx, pty_tx))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_cnb_container(
        &self,
        name: String,
        path: &Path,
        command: Option<&String>,
        env: &HashMap<String, String>,
        port: Option<u16>,
        verbose: bool,
        log_callback: Option<std::sync::Arc<dyn Fn(String) + Send + Sync>>,
        cgroup_path: Option<&str>,
    ) -> Result<PathBuf> {
        info!("Preparing containerized service {}", name);

        // 1. Setup directories
        let home = directories::UserDirs::new()
            .ok_or_else(|| anyhow::anyhow!("Failed to get user dirs"))?
            .home_dir()
            .to_path_buf();

        // Use BuilderImage to prepare the environment
        let cache_root = home.join(".local/share/locald/builders");
        let builder_image_name = "heroku/builder:22"; // TODO: Make configurable
        let builder_dir_name = builder_image_name.replace(['/', ':'], "_");
        let builder_cache_dir = cache_root.join(&builder_dir_name);

        let builder = BuilderImage::new(builder_image_name, &builder_cache_dir, vec![]);
        let cnb_dir = builder
            .ensure_available()
            .await
            .context("Failed to prepare builder image")?;

        let lifecycle = Lifecycle::new(&cnb_dir);

        let state_dir = locald_utils::project::get_state_dir(path);
        let build_dir = state_dir.join("build");
        let cache_dir = state_dir.join("cache");

        // Clean up previous build artifacts
        // We don't check exists() first because it might return false for permission errors
        if let Err(e) = tokio::fs::remove_dir_all(&build_dir).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(
                    "Failed to clean build dir: {}. Attempting privileged cleanup...",
                    e
                );
                ShimRuntime::cleanup_path(&build_dir)
                    .await
                    .context("Failed to clean build dir (privileged)")?;
            }
        }
        tokio::fs::create_dir_all(&build_dir).await?;

        let rootfs = build_dir.join("rootfs");

        // 2. Build
        info!("Building service {}...", name);

        lifecycle
            .run_creator(
                path,
                &format!("locald-{}", name.replace(':', "-")),
                &cache_dir,
                &build_dir,
                verbose,
                log_callback,
            )
            .await
            .context("Failed to build service")?;

        // Fetch run image environment (e.g. PATH)
        let run_image_env = lifecycle
            .get_run_image_env(&build_dir)
            .await
            .unwrap_or_default();

        // 3. Unpack
        info!("Unpacking service {}...", name);
        let image_name = format!("locald-{}", name.replace(':', "-"));
        let layout_path = build_dir
            .join("oci-layout")
            .join("index.docker.io")
            .join("library")
            .join(&image_name)
            .join("latest");

        let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let bundle_source = LocalLayoutBundleSource {
            layout_path: layout_path.clone(),
            image_ref: "latest".to_string(),
            app_dir: abs_path,
        };

        let bundle_info = bundle_source.prepare_rootfs(&build_dir).await?;

        // Fetch labels
        let labels = oci_layout::get_image_labels("latest", &layout_path)
            .await
            .unwrap_or_default();

        tracing::info!("Available labels: {:?}", labels.keys());

        // 4. Generate Config
        let cmd_args = if let Some(command_str) = command {
            vec![
                "/cnb/lifecycle/launcher".to_string(),
                "/bin/sh".to_string(),
                "-c".to_string(),
                (*command_str).clone(),
            ]
        } else {
            // Ensure metadata is available for the launcher
            let metadata_path = rootfs.join("layers/config/metadata.toml");
            if !metadata_path.exists() {
                // Try to restore from label
                let metadata_json = labels
                    .get("io.buildpacks.build.metadata")
                    .or_else(|| labels.get("io.buildpacks.lifecycle.metadata"));

                if let Some(json_str) = metadata_json {
                    tracing::info!("Restoring metadata.toml from image label");
                    if let Ok(val) = serde_json::from_str::<toml::Value>(json_str) {
                        if let Some(parent) = metadata_path.parent() {
                            let _ = tokio::fs::create_dir_all(parent).await;
                        }
                        if let Ok(toml_str) = toml::to_string_pretty(&val) {
                            let _ = tokio::fs::write(&metadata_path, toml_str).await;
                        }
                    } else {
                        tracing::warn!(
                            "Failed to parse metadata label as TOML compatible structure"
                        );
                    }
                }
            }

            vec!["/cnb/lifecycle/launcher".to_string()]
        };

        let mut env_vec = vec![
            "CNB_PLATFORM_API=0.12".to_string(),
            "CNB_EXPERIMENTAL_MODE=warn".to_string(),
            "CNB_LAYERS_DIR=/layers".to_string(),
            "CNB_APP_DIR=/workspace".to_string(),
        ];

        // Add run image environment (base)
        env_vec.extend(run_image_env);

        // Add image environment (overrides)
        env_vec.extend(bundle_info.env);

        for (k, v) in env {
            env_vec.push(format!("{k}={v}"));
        }
        if let Some(p) = port {
            env_vec.push(format!("PORT={p}"));
        }
        env_vec.push(format!(
            "NOTIFY_SOCKET={}",
            self.notify_socket_path.display()
        ));

        let uid = nix::unistd::getuid().as_raw();
        let gid = nix::unistd::getgid().as_raw();

        let spec = runtime_spec::generate_config(
            std::path::Path::new("rootfs"),
            &cmd_args,
            &env_vec,
            &bundle_info.bind_mounts,
            uid,
            gid,
            0, // Run as root inside container for now
            0,
            None,
            cgroup_path,
        )?;

        let bundle_dir = build_dir.clone();
        let config_path = bundle_dir.join("config.json");
        let json_str = serde_json::to_string_pretty(&spec)?;
        tokio::fs::write(&config_path, json_str).await?;

        Ok(bundle_dir)
    }

    pub fn start_container_process(
        &self,
        name: String,
        bundle_dir: &Path,
    ) -> Result<ProcessHandle> {
        Self::spawn_bundle_process(name, bundle_dir)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_cnb_container(
        &self,
        name: String,
        path: &Path,
        command: Option<&String>,
        env: &HashMap<String, String>,
        port: Option<u16>,
        verbose: bool,
        event_tx: Option<mpsc::Sender<locald_core::ipc::BootEvent>>,
    ) -> Result<ProcessHandle> {
        #[allow(clippy::option_if_let_else)]
        let log_callback = if let Some(tx) = event_tx {
            let name = name.clone();
            Some(std::sync::Arc::new(move |line: String| {
                let tx = tx.clone();
                let name = name.clone();
                tokio::spawn(async move {
                    let _ = tx
                        .send(locald_core::ipc::BootEvent::Log {
                            id: name,
                            line,
                            stream: locald_core::ipc::LogStream::Stdout,
                        })
                        .await;
                });
            })
                as std::sync::Arc<dyn Fn(String) + Send + Sync>)
        } else {
            None
        };

        let bundle_dir = self
            .prepare_cnb_container(
                name.clone(),
                path,
                command,
                env,
                port,
                verbose,
                log_callback,
                None,
            )
            .await?;

        // 5. Run via Shim
        Self::spawn_bundle_process(name, &bundle_dir)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_container(
        &self,
        name: String,
        image: String,
        command: Option<String>,
        env: &HashMap<String, String>,
        port: Option<u16>,
        path: &Path,
        cgroup_path: Option<&str>,
    ) -> Result<PathBuf> {
        info!("Preparing container service {} from image {}", name, image);

        // 1. Setup directories
        let home = directories::UserDirs::new()
            .ok_or_else(|| anyhow::anyhow!("Failed to get user dirs"))?
            .home_dir()
            .to_path_buf();

        let cache_root = home.join(".local/share/locald/images");
        let image_dir_name = image.replace(['/', ':'], "_");
        let image_cache_dir = cache_root.join(&image_dir_name);

        let state_dir = locald_utils::project::get_state_dir(path);
        let bundle_dir = state_dir.join("containers").join(&name);

        // 2. Prepare Bundle
        let container_image = ContainerImage::new(&image, &image_cache_dir);
        let bundle_info = container_image.prepare_rootfs(&bundle_dir).await?;

        // 3. Generate Config
        let cmd_args = command.map_or_else(
            || {
                bundle_info
                    .command
                    .unwrap_or_else(|| vec!["/bin/sh".to_string()])
            },
            |cmd_str| vec!["/bin/sh".to_string(), "-c".to_string(), cmd_str],
        );

        let mut env_vec = Vec::new();
        env_vec.extend(bundle_info.env);

        for (k, v) in env {
            env_vec.push(format!("{k}={v}"));
        }
        if let Some(p) = port {
            env_vec.push(format!("PORT={p}"));
        }
        env_vec.push(format!(
            "NOTIFY_SOCKET={}",
            self.notify_socket_path.display()
        ));

        let uid = nix::unistd::getuid().as_raw();
        let gid = nix::unistd::getgid().as_raw();

        let spec = runtime_spec::generate_config(
            std::path::Path::new("rootfs"),
            &cmd_args,
            &env_vec,
            &bundle_info.bind_mounts,
            uid,
            gid,
            0,
            0,
            bundle_info.workdir.as_deref(),
            cgroup_path,
        )?;

        let config_path = bundle_dir.join("config.json");
        let json_str = serde_json::to_string_pretty(&spec)?;
        tokio::fs::write(&config_path, json_str).await?;

        Ok(bundle_dir)
    }

    pub async fn start_container(
        &self,
        name: String,
        image: String,
        command: Option<String>,
        env: &HashMap<String, String>,
        port: Option<u16>,
        path: &Path,
    ) -> Result<ProcessHandle> {
        let bundle_dir = self
            .prepare_container(name.clone(), image, command, env, port, path, None)
            .await?;
        // 4. Run via Shim
        Self::spawn_bundle_process(name, &bundle_dir)
    }

    pub async fn terminate_process(child: &mut Box<dyn Child + Send>, name: &str, signal: Signal) {
        locald_utils::process::terminate_gracefully(child, name, signal).await;
    }
}
