//! Shim daemon client for socket-based privilege delegation.
//!
//! This module provides `ShimClient`, which connects to the shim daemon's Unix socket
//! and delegates privileged operations. This is used when running inside containers
//! (Toolbx, Distrobox) where the setuid shim is not available.
//!
//! The protocol uses length-prefixed JSON over a Unix socket.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::{BufReader, BufWriter, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Maximum payload size: 1 MiB (matches locald-shim/src/protocol.rs)
const MAX_PAYLOAD_SIZE: u32 = 1_048_576;

/// Current protocol version for wire format compatibility.
const PROTOCOL_VERSION: u32 = 1;

/// Default connection timeout
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Retry interval when waiting for daemon to start
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

// ============================================================================
// Protocol types (duplicated from locald-shim/src/protocol.rs)
//
// These are duplicated here because locald-shim cannot depend on locald-utils
// (it's a leaf node). A future refactoring could extract these to a shared
// locald-protocol crate.
// ============================================================================

/// Error codes returned by the shim daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u32)]
pub enum ErrorCode {
    /// Success
    Ok = 0,
    /// Request type not recognized
    UnknownRequest = 1,
    /// Client UID doesn't match daemon's expected UID
    PermissionDenied = 2,
    /// Protocol version not supported
    VersionIncompatible = 3,
    /// Operation failed (details in message)
    OperationFailed = 4,
    /// JSON parse error or invalid field values
    InvalidPayload = 5,
    /// Daemon is shutting down, retry with new daemon
    ShuttingDown = 6,
    /// Client is newer; daemon will shut down for restart
    RecycleDaemon = 7,
}

impl ErrorCode {
    /// Convert a u32 to an `ErrorCode`.
    #[must_use]
    pub const fn from_u32(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Ok),
            1 => Some(Self::UnknownRequest),
            2 => Some(Self::PermissionDenied),
            3 => Some(Self::VersionIncompatible),
            4 => Some(Self::OperationFailed),
            5 => Some(Self::InvalidPayload),
            6 => Some(Self::ShuttingDown),
            7 => Some(Self::RecycleDaemon),
            _ => None,
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ok => write!(f, "OK"),
            Self::UnknownRequest => write!(f, "UNKNOWN_REQUEST"),
            Self::PermissionDenied => write!(f, "PERMISSION_DENIED"),
            Self::VersionIncompatible => write!(f, "VERSION_INCOMPATIBLE"),
            Self::OperationFailed => write!(f, "OPERATION_FAILED"),
            Self::InvalidPayload => write!(f, "INVALID_PAYLOAD"),
            Self::ShuttingDown => write!(f, "SHUTTING_DOWN"),
            Self::RecycleDaemon => write!(f, "RECYCLE_DAEMON"),
        }
    }
}

/// Handshake message sent by the client as the first message on a connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Handshake {
    /// Wire format version (currently 1)
    pub protocol_version: u32,
    /// Semantic version of the client (e.g., "0.6.0")
    pub client_version: String,
}

impl Handshake {
    /// Create a new handshake with the current protocol version.
    #[must_use]
    pub fn new(client_version: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            client_version: client_version.into(),
        }
    }
}

/// Host entry for /etc/hosts synchronization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostEntry {
    /// IP address (e.g., "127.0.0.1")
    pub ip: String,
    /// Hostname (e.g., "myapp.localhost")
    pub hostname: String,
}

/// Cgroup setup strategy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CgroupStrategy {
    /// Use systemd slice delegation
    Systemd,
    /// Direct cgroup v2 manipulation
    Direct,
    /// Auto-detect best strategy
    Auto,
}

/// Request messages sent from client to daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShimRequest {
    /// Keepalive ping
    Ping,

    /// Synchronize /etc/hosts with the provided entries
    HostsSync {
        /// List of host entries to add/update
        entries: Vec<HostEntry>,
    },

    /// Set up cgroup hierarchy for locald
    CgroupSetup {
        /// Strategy to use for setup
        strategy: CgroupStrategy,
    },

    /// Kill all processes in a cgroup and remove it
    CgroupKill {
        /// Absolute cgroup path (e.g., "/locald.slice/service.scope")
        path: String,
    },

    /// Bind a privileged port (< 1024)
    BindPrivilegedPort {
        /// Port number to bind
        port: u16,
    },

    /// Install a CA certificate into the system trust store
    TrustInstall {
        /// PEM-encoded CA certificate
        ca_pem: String,
    },

    /// Request graceful daemon shutdown
    Shutdown,

    /// Refresh privileges (e.g., after an update)
    RefreshPrivileges,
}

/// Response payload for specific request types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponsePayload {
    /// Response to Ping request
    Pong {
        /// Version of the daemon
        daemon_version: String,
        /// Protocol version supported by daemon
        protocol_version: u32,
    },

    /// Port binding is ready (FD will be sent via `SCM_RIGHTS`)
    PortReady,

    /// Empty payload for operations that just succeed/fail
    Empty,
}

/// Response message from daemon to client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShimResponse {
    /// Error code (0 = success)
    pub code: u32,
    /// Human-readable message (primarily for errors)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Response data (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<ResponsePayload>,
}

impl ShimResponse {
    /// Check if this response indicates success.
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        self.code == ErrorCode::Ok as u32
    }

    /// Get the error code as an enum.
    #[must_use]
    pub const fn error_code(&self) -> Option<ErrorCode> {
        ErrorCode::from_u32(self.code)
    }
}

// ============================================================================
// Wire format helpers
// ============================================================================

/// Errors that can occur during protocol operations.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// I/O error during read/write
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Payload exceeds maximum size
    #[error("Payload too large: {size} bytes (max {MAX_PAYLOAD_SIZE})")]
    PayloadTooLarge {
        /// The size of the payload that was too large.
        size: u32,
    },

    /// Connection closed unexpectedly
    #[error("Connection closed")]
    ConnectionClosed,
}

/// Read a length-prefixed JSON message from a reader.
///
/// Returns `None` if the connection is cleanly closed (zero bytes read for length).
fn read_message<R: Read, T: for<'de> Deserialize<'de>>(
    reader: &mut R,
) -> Result<Option<T>, ProtocolError> {
    // Read 4-byte length prefix
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Ok(None);
        }
        Err(e) => return Err(ProtocolError::Io(e)),
    }

    let len = u32::from_be_bytes(len_buf);

    // Validate payload size
    if len > MAX_PAYLOAD_SIZE {
        return Err(ProtocolError::PayloadTooLarge { size: len });
    }

    // Read payload
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload)?;

    // Deserialize JSON
    let message = serde_json::from_slice(&payload)?;
    Ok(Some(message))
}

/// Write a length-prefixed JSON message to a writer.
fn write_message<W: Write, T: Serialize>(writer: &mut W, message: &T) -> Result<(), ProtocolError> {
    // Serialize to JSON
    let payload = serde_json::to_vec(message)?;

    // Check payload size
    let len = payload.len();
    if len > MAX_PAYLOAD_SIZE as usize {
        return Err(ProtocolError::PayloadTooLarge { size: len as u32 });
    }

    // Write length prefix
    let len_bytes = (len as u32).to_be_bytes();
    writer.write_all(&len_bytes)?;

    // Write payload
    writer.write_all(&payload)?;
    writer.flush()?;

    Ok(())
}

// ============================================================================
// ShimClient
// ============================================================================

/// Get the default socket directory path (~/.locald).
fn socket_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    Ok(PathBuf::from(home).join(".locald"))
}

/// Get the default socket path (~/.locald/shim.sock).
///
/// # Errors
///
/// Returns an error if the HOME environment variable is not set.
pub fn socket_path() -> Result<PathBuf> {
    Ok(socket_dir()?.join("shim.sock"))
}

/// Client for communicating with the shim daemon over a Unix socket.
///
/// This struct manages a connection to the shim daemon and provides methods
/// for each privileged operation.
#[derive(Debug)]
pub struct ShimClient {
    reader: BufReader<UnixStream>,
    writer: BufWriter<UnixStream>,
    daemon_version: String,
}

impl ShimClient {
    /// Connect to the shim daemon at the default socket path.
    ///
    /// This performs the initial handshake and validates protocol compatibility.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket cannot be connected or the handshake fails.
    pub fn connect() -> Result<Self> {
        let path = socket_path()?;
        Self::connect_to(&path)
    }

    /// Connect to the shim daemon at a specific socket path.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket cannot be connected or the handshake fails.
    pub fn connect_to(socket: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket)
            .with_context(|| format!("Failed to connect to shim socket at {}", socket.display()))?;

        stream.set_read_timeout(Some(CONNECT_TIMEOUT))?;
        stream.set_write_timeout(Some(CONNECT_TIMEOUT))?;

        let reader = BufReader::new(stream.try_clone()?);
        let writer = BufWriter::new(stream);

        let mut client = Self {
            reader,
            writer,
            daemon_version: String::new(),
        };

        // Perform handshake
        client.handshake()?;

        Ok(client)
    }

    /// Connect to the shim daemon with retries.
    ///
    /// This is useful when starting the daemon and waiting for it to become ready.
    /// Retries for up to `timeout` duration.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection cannot be established within the timeout.
    pub fn connect_with_retry(timeout: Duration) -> Result<Self> {
        let path = socket_path()?;
        Self::connect_to_with_retry(&path, timeout)
    }

    /// Connect to the shim daemon at a specific socket path with retries.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection cannot be established within the timeout.
    pub fn connect_to_with_retry(socket: &Path, timeout: Duration) -> Result<Self> {
        let start = std::time::Instant::now();

        loop {
            match Self::connect_to(socket) {
                Ok(client) => return Ok(client),
                Err(e) => {
                    if start.elapsed() >= timeout {
                        return Err(e).context(format!(
                            "Timed out waiting for shim daemon at {} (waited {:?})",
                            socket.display(),
                            timeout
                        ));
                    }
                    // This is synchronous polling, acceptable for startup
                    #[allow(clippy::disallowed_methods)]
                    std::thread::sleep(RETRY_INTERVAL);
                }
            }
        }
    }

    /// Perform the initial handshake with the daemon.
    fn handshake(&mut self) -> Result<()> {
        let client_version = env!("CARGO_PKG_VERSION");
        let handshake = Handshake::new(client_version);

        write_message(&mut self.writer, &handshake).context("Failed to send handshake")?;

        let response: ShimResponse = read_message(&mut self.reader)
            .context("Failed to read handshake response")?
            .ok_or_else(|| anyhow::anyhow!("Connection closed during handshake"))?;

        match response.error_code() {
            Some(ErrorCode::Ok) => {
                // Extract daemon version from pong payload
                if let Some(ResponsePayload::Pong {
                    daemon_version,
                    protocol_version,
                }) = response.payload
                {
                    if protocol_version != PROTOCOL_VERSION {
                        bail!(
                            "Protocol version mismatch: client={PROTOCOL_VERSION}, daemon={protocol_version}"
                        );
                    }
                    self.daemon_version = daemon_version;
                }
                Ok(())
            }
            Some(ErrorCode::RecycleDaemon) => {
                // Daemon is older than client, needs restart
                // Send shutdown and let caller retry
                // We don't care if this fails; the daemon will eventually time out
                let _ignored: Result<(), _> =
                    write_message(&mut self.writer, &ShimRequest::Shutdown);
                bail!("Daemon version mismatch, daemon is shutting down for restart")
            }
            Some(ErrorCode::VersionIncompatible) => {
                bail!(
                    "Protocol version incompatible: {}",
                    response.message.unwrap_or_default()
                )
            }
            Some(code) => {
                bail!(
                    "Handshake failed: {} - {}",
                    code,
                    response.message.unwrap_or_default()
                )
            }
            None => {
                bail!(
                    "Handshake failed with unknown error code: {}",
                    response.code
                )
            }
        }
    }

    /// Send a request and wait for a response.
    fn request(&mut self, req: &ShimRequest) -> Result<ShimResponse> {
        write_message(&mut self.writer, req).context("Failed to send request")?;

        let response: ShimResponse = read_message(&mut self.reader)
            .context("Failed to read response")?
            .ok_or_else(|| anyhow::anyhow!("Connection closed while waiting for response"))?;

        Ok(response)
    }

    /// Get the daemon version (populated after handshake).
    #[must_use]
    pub fn daemon_version(&self) -> &str {
        &self.daemon_version
    }

    /// Send a ping request to verify the daemon is responsive.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the daemon returns an error.
    pub fn ping(&mut self) -> Result<()> {
        let response = self.request(&ShimRequest::Ping)?;
        if response.is_ok() {
            Ok(())
        } else {
            bail!(
                "Ping failed: {}",
                response
                    .message
                    .unwrap_or_else(|| "unknown error".to_string())
            )
        }
    }

    /// Synchronize /etc/hosts with the provided entries.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the daemon returns an error.
    pub fn hosts_sync(&mut self, entries: Vec<HostEntry>) -> Result<()> {
        let response = self.request(&ShimRequest::HostsSync { entries })?;
        if response.is_ok() {
            Ok(())
        } else {
            bail!(
                "Hosts sync failed: {}",
                response
                    .message
                    .unwrap_or_else(|| "unknown error".to_string())
            )
        }
    }

    /// Set up cgroup hierarchy for locald.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the daemon returns an error.
    pub fn cgroup_setup(&mut self, strategy: CgroupStrategy) -> Result<()> {
        let response = self.request(&ShimRequest::CgroupSetup { strategy })?;
        if response.is_ok() {
            Ok(())
        } else {
            bail!(
                "Cgroup setup failed: {}",
                response
                    .message
                    .unwrap_or_else(|| "unknown error".to_string())
            )
        }
    }

    /// Kill all processes in a cgroup and remove it.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the daemon returns an error.
    pub fn cgroup_kill(&mut self, path: impl Into<String>) -> Result<()> {
        let response = self.request(&ShimRequest::CgroupKill { path: path.into() })?;
        if response.is_ok() {
            Ok(())
        } else {
            bail!(
                "Cgroup kill failed: {}",
                response
                    .message
                    .unwrap_or_else(|| "unknown error".to_string())
            )
        }
    }

    /// Install a CA certificate into the system trust store.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the daemon returns an error.
    pub fn trust_install(&mut self, ca_pem: impl Into<String>) -> Result<()> {
        let response = self.request(&ShimRequest::TrustInstall {
            ca_pem: ca_pem.into(),
        })?;
        if response.is_ok() {
            Ok(())
        } else {
            bail!(
                "Trust install failed: {}",
                response
                    .message
                    .unwrap_or_else(|| "unknown error".to_string())
            )
        }
    }

    /// Request graceful daemon shutdown.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn shutdown(&mut self) -> Result<()> {
        let response = self.request(&ShimRequest::Shutdown)?;
        if response.is_ok() || matches!(response.error_code(), Some(ErrorCode::ShuttingDown)) {
            Ok(())
        } else {
            bail!(
                "Shutdown request failed: {}",
                response
                    .message
                    .unwrap_or_else(|| "unknown error".to_string())
            )
        }
    }
}

/// Check if the shim socket exists and is connectable.
///
/// This is a quick check without fully connecting (just checks socket existence).
#[must_use]
pub fn socket_exists() -> bool {
    socket_path().is_ok_and(|p| p.exists())
}

/// Check if we can connect to the shim daemon.
///
/// Attempts a quick connection and ping.
pub fn can_connect() -> bool {
    ShimClient::connect()
        .and_then(|mut client| client.ping())
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handshake_serialization() {
        let handshake = Handshake::new("0.6.0");
        let json = serde_json::to_string(&handshake).unwrap();
        assert!(json.contains("protocol_version"));
        assert!(json.contains("client_version"));
        assert!(json.contains("0.6.0"));

        let parsed: Handshake = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, handshake);
    }

    #[test]
    fn test_shim_request_ping() {
        let req = ShimRequest::Ping;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"ping"}"#);

        let parsed: ShimRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, req);
    }

    #[test]
    fn test_shim_request_hosts_sync() {
        let req = ShimRequest::HostsSync {
            entries: vec![HostEntry {
                ip: "127.0.0.1".to_string(),
                hostname: "myapp.localhost".to_string(),
            }],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("hosts_sync"));
        assert!(json.contains("127.0.0.1"));
        assert!(json.contains("myapp.localhost"));

        let parsed: ShimRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, req);
    }

    #[test]
    fn test_error_code_roundtrip() {
        for code in [
            ErrorCode::Ok,
            ErrorCode::UnknownRequest,
            ErrorCode::PermissionDenied,
            ErrorCode::VersionIncompatible,
            ErrorCode::OperationFailed,
            ErrorCode::InvalidPayload,
            ErrorCode::ShuttingDown,
            ErrorCode::RecycleDaemon,
        ] {
            let n = code as u32;
            let back = ErrorCode::from_u32(n).unwrap();
            assert_eq!(code, back);
        }
    }

    #[test]
    fn test_socket_path() {
        // This test just verifies the function doesn't panic when HOME is set
        if std::env::var("HOME").is_ok() {
            let path = socket_path().unwrap();
            assert!(path.ends_with("shim.sock"));
            assert!(path.to_string_lossy().contains(".locald"));
        }
    }
}
