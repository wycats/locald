//! Shim daemon wire protocol.
//!
//! This module defines the length-prefixed JSON wire format used for communication
//! between the locald client and the shim daemon over a Unix socket.
//!
//! ## Wire Format
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────┐
//! │  4 bytes (big-endian u32)  │  N bytes (UTF-8 JSON payload)  │
//! │       payload length       │         Message                │
//! └──────────────────────────────────────────────────────────────┘
//! ```
//!
//! - **Length prefix**: 4 bytes, big-endian unsigned 32-bit integer
//! - **Payload**: UTF-8 encoded JSON, exactly `length` bytes
//! - **Max payload size**: 1 MiB (1,048,576 bytes)

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Maximum payload size: 1 MiB
pub const MAX_PAYLOAD_SIZE: u32 = 1_048_576;

/// Current protocol version for wire format compatibility.
pub const PROTOCOL_VERSION: u32 = 1;

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
    /// Convert a u32 to an ErrorCode.
    #[must_use]
    #[allow(dead_code)]
    pub fn from_u32(code: u32) -> Option<Self> {
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

    /// Get the numeric code.
    #[must_use]
    pub fn as_u32(self) -> u32 {
        self as u32
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
    #[allow(dead_code)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

    /// Port binding is ready (FD will be sent via SCM_RIGHTS)
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
    /// Create a successful response with a payload.
    #[must_use]
    pub fn ok(payload: ResponsePayload) -> Self {
        Self {
            code: ErrorCode::Ok.as_u32(),
            message: Some("OK".to_string()),
            payload: Some(payload),
        }
    }

    /// Create a successful response with no payload.
    #[must_use]
    pub fn ok_empty() -> Self {
        Self {
            code: ErrorCode::Ok.as_u32(),
            message: Some("OK".to_string()),
            payload: None,
        }
    }

    /// Create an error response.
    #[must_use]
    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code.as_u32(),
            message: Some(message.into()),
            payload: None,
        }
    }

    /// Create a pong response.
    #[must_use]
    pub fn pong(daemon_version: impl Into<String>) -> Self {
        Self::ok(ResponsePayload::Pong {
            daemon_version: daemon_version.into(),
            protocol_version: PROTOCOL_VERSION,
        })
    }

    /// Check if this response indicates success.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.code == ErrorCode::Ok.as_u32()
    }

    /// Get the error code as an enum.
    #[must_use]
    #[allow(dead_code)]
    pub fn error_code(&self) -> Option<ErrorCode> {
        ErrorCode::from_u32(self.code)
    }
}

/// Errors that can occur during protocol operations.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// I/O error during read/write
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// JSON serialization/deserialization error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Payload exceeds maximum size
    #[error("Payload too large: {size} bytes (max {MAX_PAYLOAD_SIZE})")]
    PayloadTooLarge { size: u32 },

    /// Connection closed unexpectedly
    #[error("Connection closed")]
    #[allow(dead_code)]
    ConnectionClosed,
}

/// Read a length-prefixed JSON message from a reader.
///
/// Returns `None` if the connection is cleanly closed (zero bytes read for length).
pub fn read_message<R: Read, T: for<'de> Deserialize<'de>>(
    reader: &mut R,
) -> Result<Option<T>, ProtocolError> {
    // Read 4-byte length prefix
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
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
pub fn write_message<W: Write, T: Serialize>(
    writer: &mut W,
    message: &T,
) -> Result<(), ProtocolError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

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
    fn test_shim_request_cgroup_setup() {
        let req = ShimRequest::CgroupSetup {
            strategy: CgroupStrategy::Auto,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("cgroup_setup"));
        assert!(json.contains("auto"));

        let parsed: ShimRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, req);
    }

    #[test]
    fn test_shim_request_cgroup_kill() {
        let req = ShimRequest::CgroupKill {
            path: "/locald.slice/service.scope".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("cgroup_kill"));
        assert!(json.contains("/locald.slice/service.scope"));

        let parsed: ShimRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, req);
    }

    #[test]
    fn test_shim_response_ok() {
        let resp = ShimResponse::ok_empty();
        assert!(resp.is_ok());
        assert_eq!(resp.code, 0);
    }

    #[test]
    fn test_shim_response_pong() {
        let resp = ShimResponse::pong("0.6.0");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("pong"));
        assert!(json.contains("0.6.0"));

        let parsed: ShimResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.code, 0);
        assert!(matches!(parsed.payload, Some(ResponsePayload::Pong { .. })));
    }

    #[test]
    fn test_shim_response_error() {
        let resp = ShimResponse::error(ErrorCode::PermissionDenied, "Access denied");
        assert!(!resp.is_ok());
        assert_eq!(resp.code, ErrorCode::PermissionDenied.as_u32());
        assert_eq!(resp.message.as_deref(), Some("Access denied"));
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
            let n = code.as_u32();
            let back = ErrorCode::from_u32(n).unwrap();
            assert_eq!(code, back);
        }
    }

    #[test]
    fn test_wire_format_roundtrip() {
        let original = ShimRequest::Ping;

        // Write to buffer
        let mut buffer = Vec::new();
        write_message(&mut buffer, &original).unwrap();

        // Verify length prefix
        let len = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
        assert_eq!(len as usize, buffer.len() - 4);

        // Read back
        let mut cursor = Cursor::new(&buffer);
        let parsed: ShimRequest = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn test_wire_format_handshake() {
        let original = Handshake::new("1.2.3");

        let mut buffer = Vec::new();
        write_message(&mut buffer, &original).unwrap();

        let mut cursor = Cursor::new(&buffer);
        let parsed: Handshake = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn test_wire_format_complex_request() {
        let original = ShimRequest::HostsSync {
            entries: vec![
                HostEntry {
                    ip: "127.0.0.1".to_string(),
                    hostname: "app1.localhost".to_string(),
                },
                HostEntry {
                    ip: "127.0.0.1".to_string(),
                    hostname: "app2.localhost".to_string(),
                },
            ],
        };

        let mut buffer = Vec::new();
        write_message(&mut buffer, &original).unwrap();

        let mut cursor = Cursor::new(&buffer);
        let parsed: ShimRequest = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn test_read_empty_stream_returns_none() {
        let buffer: Vec<u8> = vec![];
        let mut cursor = Cursor::new(&buffer);
        let result: Result<Option<ShimRequest>, _> = read_message(&mut cursor);
        assert!(matches!(result, Ok(None)));
    }

    #[test]
    fn test_payload_too_large() {
        // Create a fake length prefix indicating a huge payload
        let fake_len = (MAX_PAYLOAD_SIZE + 1).to_be_bytes();
        let mut cursor = Cursor::new(fake_len.to_vec());
        let result: Result<Option<ShimRequest>, _> = read_message(&mut cursor);
        assert!(matches!(result, Err(ProtocolError::PayloadTooLarge { .. })));
    }
}
