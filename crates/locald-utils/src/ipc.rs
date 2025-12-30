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
