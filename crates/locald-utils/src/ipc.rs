//! IPC utilities.

use crate::env::is_sandbox_active;
use std::path::PathBuf;
use thiserror::Error;

/// Errors related to IPC configuration.
#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    /// `LOCALD_SOCKET` is set but `LOCALD_SANDBOX_ACTIVE` is not.
    #[error("LOCALD_SOCKET is not allowed without LOCALD_SANDBOX_ACTIVE")]
    SocketEnvNotAllowed,
}

/// Returns the path to the locald IPC socket.
///
/// # Errors
///
/// Returns `IpcError::SocketEnvNotAllowed` if `LOCALD_SOCKET` is set but `LOCALD_SANDBOX_ACTIVE` is not.
pub fn socket_path() -> Result<PathBuf, IpcError> {
    let socket_env = std::env::var("LOCALD_SOCKET");

    if socket_env.is_ok() && !is_sandbox_active() {
        return Err(IpcError::SocketEnvNotAllowed);
    }

    socket_env.map_or_else(
        |_| Ok(PathBuf::from("/tmp/locald.sock")),
        |path| Ok(PathBuf::from(path)),
    )
}

/// Send a request to the locald daemon and return the response.
///
/// This is a basic blocking IPC client. For richer error handling
/// (distinguishing not-running vs connection-refused, etc.), use
/// the CLI's `client::send_request` instead.
///
/// # Errors
///
/// Returns an error if the socket cannot be connected to, or if
/// serialization/deserialization fails.
pub fn send_request(request: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    let path = socket_path()?;
    let mut stream = UnixStream::connect(&path)?;
    stream.write_all(request)?;

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf)?;
    Ok(buf)
}
