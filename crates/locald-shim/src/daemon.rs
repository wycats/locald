//! Shim daemon implementation.
//!
//! This module implements the `ShimDaemon` which listens on a Unix socket
//! for privileged operation requests from the locald client.
//!
//! ## Lifecycle
//!
//! 1. The daemon binds to `~/.locald/shim.sock` before forking
//! 2. It writes its PID to `~/.locald/shim.pid`
//! 3. It accepts connections and handles requests
//! 4. It shuts down on SIGTERM/SIGINT, idle timeout (5 min), or max lifetime (1 hour)

// The shim daemon intentionally uses synchronous/blocking I/O since it's a simple
// single-threaded daemon that doesn't need async complexity. The tokio-based clippy
// rules don't apply here.
#![allow(clippy::disallowed_methods)]

use crate::protocol::{
    self, ErrorCode, Handshake, PROTOCOL_VERSION, ProtocolError, ResponsePayload, ShimRequest,
    ShimResponse,
};
use anyhow::{Context, Result};
use nix::unistd::Uid;
use std::fs::{self, File, Permissions};
use std::io::{BufReader, BufWriter, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Default idle timeout (5 minutes).
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Maximum daemon lifetime (1 hour).
pub const MAX_LIFETIME: Duration = Duration::from_secs(3600);

/// Graceful shutdown timeout for in-flight requests.
#[allow(dead_code)]
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Get the socket directory path (~/.locald).
fn socket_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    Ok(PathBuf::from(home).join(".locald"))
}

/// Get the socket path (~/.locald/shim.sock).
pub fn socket_path() -> Result<PathBuf> {
    Ok(socket_dir()?.join("shim.sock"))
}

/// Get the PID file path (~/.locald/shim.pid).
fn pid_path() -> Result<PathBuf> {
    Ok(socket_dir()?.join("shim.pid"))
}

/// Get the log file path (~/.locald/shim.log).
fn log_path() -> Result<PathBuf> {
    Ok(socket_dir()?.join("shim.log"))
}

/// Check if a process with the given PID exists.
fn process_exists(pid: i32) -> bool {
    // Sending signal 0 checks if process exists without actually signaling it
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

/// Configuration for the shim daemon.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Idle timeout before shutdown (default: 5 minutes)
    pub idle_timeout: Duration,
    /// Maximum daemon lifetime (default: 1 hour)
    pub max_lifetime: Duration,
    /// Run in foreground (don't daemonize)
    pub foreground: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            max_lifetime: MAX_LIFETIME,
            foreground: false,
        }
    }
}

/// The shim daemon that handles privileged operation requests.
pub struct ShimDaemon {
    /// The Unix socket listener
    listener: UnixListener,
    /// Path to the socket file
    socket_path: PathBuf,
    /// Path to the PID file
    pid_path: PathBuf,
    /// Number of currently active client connections
    active_clients: AtomicUsize,
    /// Timestamp of last client activity
    last_activity: Mutex<Instant>,
    /// Shutdown flag
    shutdown: Arc<AtomicBool>,
    /// Daemon configuration
    config: DaemonConfig,
    /// Daemon start time
    start_time: Instant,
    /// The daemon's version (for handshake responses)
    daemon_version: String,
    /// Expected UID for peer credential validation
    expected_uid: Uid,
}

impl ShimDaemon {
    /// Create a new daemon instance.
    ///
    /// This binds the socket but doesn't start accepting connections yet.
    pub fn new(config: DaemonConfig) -> Result<Self> {
        let socket_dir = socket_dir()?;
        fs::create_dir_all(&socket_dir).with_context(|| {
            format!(
                "Failed to create socket directory: {}",
                socket_dir.display()
            )
        })?;

        let socket_path = socket_path()?;
        let pid_path = pid_path()?;

        // Check for existing daemon
        if socket_path.exists() {
            if let Ok(pid_content) = fs::read_to_string(&pid_path)
                && let Ok(pid) = pid_content.trim().parse::<i32>()
                && process_exists(pid)
            {
                // Already running, return the socket path
                anyhow::bail!(
                    "Shim daemon already running (pid {}). Socket: {}",
                    pid,
                    socket_path.display()
                );
            }
            // Stale socket, remove it
            fs::remove_file(&socket_path).with_context(|| {
                format!("Failed to remove stale socket: {}", socket_path.display())
            })?;
        }

        // Remove stale PID file if it exists
        if pid_path.exists() {
            let _ = fs::remove_file(&pid_path);
        }

        // Bind the socket
        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("Failed to bind socket: {}", socket_path.display()))?;

        // Set socket permissions to owner-only (0600)
        fs::set_permissions(&socket_path, Permissions::from_mode(0o600)).with_context(|| {
            format!(
                "Failed to set socket permissions: {}",
                socket_path.display()
            )
        })?;

        // Set non-blocking for accept timeout
        listener
            .set_nonblocking(true)
            .context("Failed to set socket to non-blocking mode")?;

        let expected_uid = Uid::current();
        let daemon_version = env!("CARGO_PKG_VERSION").to_string();

        Ok(Self {
            listener,
            socket_path,
            pid_path,
            active_clients: AtomicUsize::new(0),
            last_activity: Mutex::new(Instant::now()),
            shutdown: Arc::new(AtomicBool::new(false)),
            config,
            start_time: Instant::now(),
            daemon_version,
            expected_uid,
        })
    }

    /// Get the socket path for this daemon.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Get the shutdown flag for signal handler registration.
    #[must_use]
    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown)
    }

    /// Write the PID file.
    fn write_pid_file(&self) -> Result<()> {
        let pid = std::process::id();
        fs::write(&self.pid_path, format!("{pid}\n"))
            .with_context(|| format!("Failed to write PID file: {}", self.pid_path.display()))?;
        Ok(())
    }

    /// Update the last activity timestamp.
    fn touch_activity(&self) {
        if let Ok(mut last) = self.last_activity.lock() {
            *last = Instant::now();
        }
    }

    /// Check if we should shut down due to timeouts.
    fn should_shutdown(&self) -> Option<&'static str> {
        // Check explicit shutdown flag
        if self.shutdown.load(Ordering::SeqCst) {
            return Some("shutdown requested");
        }

        // Check max lifetime
        if self.start_time.elapsed() > self.config.max_lifetime {
            return Some("max lifetime reached");
        }

        // Check idle timeout (only when no active clients)
        if self.active_clients.load(Ordering::SeqCst) == 0
            && let Ok(last) = self.last_activity.lock()
            && last.elapsed() > self.config.idle_timeout
        {
            return Some("idle timeout reached");
        }

        None
    }

    /// Run the daemon main loop.
    pub fn run(&self) -> Result<()> {
        self.write_pid_file()?;

        eprintln!(
            "Shim daemon started (pid {}, socket {})",
            std::process::id(),
            self.socket_path.display()
        );

        // Main accept loop
        loop {
            // Check shutdown conditions
            if let Some(reason) = self.should_shutdown() {
                eprintln!("Shutting down: {reason}");
                break;
            }

            // Try to accept a connection with a short timeout
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    self.touch_activity();
                    self.active_clients.fetch_add(1, Ordering::SeqCst);

                    // Handle connection (blocking, single-threaded for simplicity)
                    // In a future iteration, this could be made async or multi-threaded
                    let result = self.handle_connection(stream);

                    self.active_clients.fetch_sub(1, Ordering::SeqCst);
                    self.touch_activity();

                    if let Err(e) = result {
                        eprintln!("Connection error: {e:?}");
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No connection pending, sleep briefly
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    eprintln!("Accept error: {e}");
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }

        self.cleanup();
        Ok(())
    }

    /// Validate peer credentials for a connection.
    fn validate_peer(&self, stream: &UnixStream) -> Result<(), ShimResponse> {
        use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};

        // Use nix crate's getsockopt for peer credentials
        match getsockopt(stream, PeerCredentials) {
            Ok(cred) => {
                let peer_uid = Uid::from_raw(cred.uid());
                if peer_uid != self.expected_uid {
                    return Err(ShimResponse::error(
                        ErrorCode::PermissionDenied,
                        format!(
                            "Unauthorized client (UID {} != {})",
                            cred.uid(),
                            self.expected_uid.as_raw()
                        ),
                    ));
                }
                Ok(())
            }
            Err(e) => Err(ShimResponse::error(
                ErrorCode::PermissionDenied,
                format!("Failed to get peer credentials: {e}"),
            )),
        }
    }

    /// Handle a single client connection.
    fn handle_connection(&self, stream: UnixStream) -> Result<()> {
        // Validate peer credentials
        if let Err(response) = self.validate_peer(&stream) {
            // Try to send error response, but don't fail if we can't
            let _ = self.send_response(&stream, &response);
            return Ok(());
        }

        // Set stream to blocking mode for request handling
        stream
            .set_nonblocking(false)
            .context("Failed to set stream to blocking mode")?;

        // Set read timeout to avoid blocking forever
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .context("Failed to set read timeout")?;

        let mut reader = BufReader::new(&stream);
        let mut writer = BufWriter::new(&stream);

        // First message must be handshake
        let handshake: Handshake = match protocol::read_message(&mut reader) {
            Ok(Some(h)) => h,
            Ok(None) => return Ok(()), // Client closed connection
            Err(ProtocolError::Json(e)) => {
                let resp = ShimResponse::error(
                    ErrorCode::InvalidPayload,
                    format!("Invalid handshake JSON: {e}"),
                );
                let _ = protocol::write_message(&mut writer, &resp);
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };

        // Validate handshake
        let handshake_response = self.validate_handshake(&handshake);
        protocol::write_message(&mut writer, &handshake_response)?;

        if !handshake_response.is_ok() {
            return Ok(());
        }

        // Request/response loop
        loop {
            let request: ShimRequest = match protocol::read_message(&mut reader) {
                Ok(Some(r)) => r,
                Ok(None) => break, // Client closed connection
                Err(ProtocolError::Json(e)) => {
                    let resp = ShimResponse::error(
                        ErrorCode::InvalidPayload,
                        format!("Invalid request JSON: {e}"),
                    );
                    protocol::write_message(&mut writer, &resp)?;
                    continue;
                }
                Err(ProtocolError::Io(e)) if e.kind() == std::io::ErrorKind::TimedOut => {
                    // Read timeout, close connection
                    break;
                }
                Err(e) => return Err(e.into()),
            };

            self.touch_activity();

            // Check if shutting down
            if self.shutdown.load(Ordering::SeqCst) {
                let resp = ShimResponse::error(ErrorCode::ShuttingDown, "Daemon is shutting down");
                protocol::write_message(&mut writer, &resp)?;
                break;
            }

            // Handle the request
            let response = self.handle_request(request);

            // Check if this was a shutdown request
            let is_shutdown = response.is_ok()
                && matches!(response.payload, Some(ResponsePayload::Empty))
                && self.shutdown.load(Ordering::SeqCst);

            protocol::write_message(&mut writer, &response)?;

            if is_shutdown {
                break;
            }
        }

        Ok(())
    }

    /// Validate the client handshake.
    fn validate_handshake(&self, handshake: &Handshake) -> ShimResponse {
        // Check protocol version
        if handshake.protocol_version != PROTOCOL_VERSION {
            return ShimResponse::error(
                ErrorCode::VersionIncompatible,
                format!(
                    "Protocol version {} not supported (expected {})",
                    handshake.protocol_version, PROTOCOL_VERSION
                ),
            );
        }

        // Parse client version for comparison
        let client_version = &handshake.client_version;
        let daemon_version = &self.daemon_version;

        // Simple version comparison (could use semver crate for proper comparison)
        if is_newer_version(client_version, daemon_version) {
            // Client is newer, daemon should recycle
            self.shutdown.store(true, Ordering::SeqCst);
            return ShimResponse::error(
                ErrorCode::RecycleDaemon,
                format!(
                    "Client version {} is newer than daemon version {}; daemon will shut down",
                    client_version, daemon_version
                ),
            );
        }

        // Handshake successful - return pong with daemon info
        ShimResponse::pong(&self.daemon_version)
    }

    /// Handle a single request.
    fn handle_request(&self, request: ShimRequest) -> ShimResponse {
        match request {
            ShimRequest::Ping => ShimResponse::pong(&self.daemon_version),

            ShimRequest::HostsSync { entries } => {
                // Extract hostnames from entries - the update_hosts_file function
                // hardcodes 127.0.0.1, so we just pass the hostnames.
                let domains: Vec<String> = entries.into_iter().map(|e| e.hostname).collect();
                eprintln!("HostsSync: syncing {} entries", domains.len());
                match crate::update_hosts_file(&domains) {
                    Ok(()) => ShimResponse::ok_empty(),
                    Err(e) => {
                        eprintln!("HostsSync failed: {e:#}");
                        ShimResponse::error(ErrorCode::OperationFailed, e.to_string())
                    }
                }
            }

            ShimRequest::CgroupSetup { strategy } => {
                use crate::protocol::CgroupStrategy;
                eprintln!("CgroupSetup: strategy = {strategy:?}");
                let result = match strategy {
                    CgroupStrategy::Auto => crate::cgroup_setup(),
                    CgroupStrategy::Systemd => crate::cgroup_setup_systemd(),
                    CgroupStrategy::Direct => crate::cgroup_setup_driver(),
                };
                match result {
                    Ok(()) => ShimResponse::ok_empty(),
                    Err(e) => {
                        eprintln!("CgroupSetup failed: {e:#}");
                        ShimResponse::error(ErrorCode::OperationFailed, e.to_string())
                    }
                }
            }

            ShimRequest::CgroupKill { path } => {
                eprintln!("CgroupKill: path = {path}");
                match crate::cgroup_kill_and_prune(&path) {
                    Ok(()) => ShimResponse::ok_empty(),
                    Err(e) => {
                        eprintln!("CgroupKill failed: {e:#}");
                        ShimResponse::error(ErrorCode::OperationFailed, e.to_string())
                    }
                }
            }

            ShimRequest::BindPrivilegedPort { port } => {
                // Placeholder: just acknowledge (real impl would use SCM_RIGHTS)
                eprintln!("BindPrivilegedPort: port = {port}");
                ShimResponse::ok(ResponsePayload::PortReady)
            }

            ShimRequest::TrustInstall { ca_pem } => {
                // Placeholder: just acknowledge
                let _ = ca_pem;
                eprintln!("TrustInstall: would install CA certificate");
                ShimResponse::ok_empty()
            }

            ShimRequest::Shutdown => {
                eprintln!("Shutdown requested by client");
                self.shutdown.store(true, Ordering::SeqCst);
                ShimResponse::ok_empty()
            }

            ShimRequest::RefreshPrivileges => {
                // Placeholder: just acknowledge
                eprintln!("RefreshPrivileges: would refresh privileges");
                ShimResponse::ok_empty()
            }
        }
    }

    /// Send a response to the client.
    fn send_response(&self, stream: &UnixStream, response: &ShimResponse) -> Result<()> {
        let mut writer = BufWriter::new(stream);
        protocol::write_message(&mut writer, response)?;
        Ok(())
    }

    /// Clean up resources on shutdown.
    fn cleanup(&self) {
        eprintln!("Cleaning up...");

        // Remove socket file
        if let Err(e) = fs::remove_file(&self.socket_path) {
            eprintln!(
                "Warning: Failed to remove socket file {}: {e}",
                self.socket_path.display()
            );
        }

        // Remove PID file
        if let Err(e) = fs::remove_file(&self.pid_path) {
            eprintln!(
                "Warning: Failed to remove PID file {}: {e}",
                self.pid_path.display()
            );
        }

        eprintln!("Shutdown complete");
    }
}

impl Drop for ShimDaemon {
    fn drop(&mut self) {
        // Ensure cleanup happens even on panic
        // Note: cleanup() is idempotent (remove_file on non-existent file is fine)
        let _ = fs::remove_file(&self.socket_path);
        let _ = fs::remove_file(&self.pid_path);
    }
}

/// Simple version comparison (returns true if a > b).
///
/// This is a simplified comparison that works for semantic versions.
/// For production use, consider using the `semver` crate.
fn is_newer_version(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> { s.split('.').filter_map(|p| p.parse().ok()).collect() };

    let a_parts = parse(a);
    let b_parts = parse(b);

    for (a_part, b_part) in a_parts.iter().zip(b_parts.iter()) {
        match a_part.cmp(b_part) {
            std::cmp::Ordering::Greater => return true,
            std::cmp::Ordering::Less => return false,
            std::cmp::Ordering::Equal => continue,
        }
    }

    // If all compared parts are equal, the longer version is newer
    a_parts.len() > b_parts.len()
}

/// Start the shim daemon with the given configuration.
///
/// If `foreground` is true, runs in the current process.
/// Otherwise, forks into the background using the `daemonize` crate.
pub fn serve(config: DaemonConfig) -> Result<()> {
    // Create daemon (this binds the socket)
    let daemon = ShimDaemon::new(config.clone())?;

    // Print socket path to signal readiness (before forking)
    println!("{}", daemon.socket_path().display());

    // Flush stdout before fork
    std::io::stdout().flush()?;

    if config.foreground {
        // Set up signal handlers
        setup_signal_handlers(daemon.shutdown_flag())?;

        // Run in foreground
        daemon.run()
    } else {
        // Daemonize
        let log_path = log_path()?;
        let log_file = File::create(&log_path)
            .with_context(|| format!("Failed to create log file: {}", log_path.display()))?;

        let daemonize = daemonize::Daemonize::new()
            .pid_file(daemon.pid_path.clone())
            .chown_pid_file(true)
            .stdout(log_file.try_clone()?)
            .stderr(log_file);

        match daemonize.start() {
            Ok(()) => {
                // We're now in the daemon process
                // Set up signal handlers
                setup_signal_handlers(daemon.shutdown_flag())?;

                // Run the daemon loop
                daemon.run()
            }
            Err(e) => {
                anyhow::bail!("Failed to daemonize: {e}");
            }
        }
    }
}

/// Set up signal handlers for graceful shutdown.
fn setup_signal_handlers(shutdown: Arc<AtomicBool>) -> Result<()> {
    // Register SIGTERM handler
    signal_hook::flag::register(signal_hook::consts::SIGTERM, shutdown.clone())
        .context("Failed to register SIGTERM handler")?;

    // Register SIGINT handler (for foreground mode / Ctrl-C)
    signal_hook::flag::register(signal_hook::consts::SIGINT, shutdown)
        .context("Failed to register SIGINT handler")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer_version() {
        assert!(is_newer_version("1.0.0", "0.9.9"));
        assert!(is_newer_version("1.1.0", "1.0.9"));
        assert!(is_newer_version("1.0.1", "1.0.0"));
        assert!(is_newer_version("2.0.0", "1.9.9"));

        assert!(!is_newer_version("0.9.9", "1.0.0"));
        assert!(!is_newer_version("1.0.0", "1.0.0"));
        assert!(!is_newer_version("1.0.0", "1.0.1"));
    }

    #[test]
    fn test_is_newer_version_different_lengths() {
        assert!(is_newer_version("1.0.0.1", "1.0.0"));
        assert!(!is_newer_version("1.0.0", "1.0.0.1"));
    }

    #[test]
    fn test_socket_paths() {
        // Just ensure these don't panic
        let _ = socket_path();
        let _ = pid_path();
        let _ = log_path();
    }
}
