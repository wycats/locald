use miette::Diagnostic;
use thiserror::Error;

pub type CliResult<T> = Result<T, CliError>;

#[derive(Error, Debug, Diagnostic)]
pub enum CliError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Daemon(#[from] DaemonError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Config(#[from] ConfigError),

    #[error("{message}")]
    #[diagnostic(code(locald::cli::error))]
    Other { message: String },
}

impl CliError {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Other {
            message: message.into(),
        }
    }
}

impl From<anyhow::Error> for CliError {
    fn from(err: anyhow::Error) -> Self {
        Self::message(err.to_string())
    }
}

impl From<std::io::Error> for CliError {
    fn from(err: std::io::Error) -> Self {
        Self::message(err.to_string())
    }
}

impl From<std::env::VarError> for CliError {
    fn from(err: std::env::VarError) -> Self {
        Self::message(err.to_string())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(err: serde_json::Error) -> Self {
        Self::message(err.to_string())
    }
}

impl From<toml::de::Error> for CliError {
    fn from(err: toml::de::Error) -> Self {
        Self::message(err.to_string())
    }
}

impl From<toml::ser::Error> for CliError {
    fn from(err: toml::ser::Error) -> Self {
        Self::message(err.to_string())
    }
}

#[allow(dead_code)]
#[derive(Error, Debug, Diagnostic)]
pub enum ConfigError {
    #[error("{message}")]
    #[diagnostic(code(locald::config))]
    Generic { message: String },
}

impl ConfigError {
    #[allow(dead_code)]
    pub fn new(message: impl Into<String>) -> Self {
        Self::Generic {
            message: message.into(),
        }
    }
}

#[derive(Error, Debug, Diagnostic)]
pub enum DaemonError {
    #[error("locald is not running (socket not found at {socket_path})")]
    #[diagnostic(
        code(locald::daemon::not_running),
        help("Run `locald up` to start the daemon (socket: {socket_path}).")
    )]
    NotRunning { socket_path: String },

    #[error("locald is not running (connection refused at {socket_path})")]
    #[diagnostic(
        code(locald::daemon::connection_refused),
        help("Run `locald up` to start the daemon (socket: {socket_path}).")
    )]
    ConnectionRefused { socket_path: String },

    #[error("Permission denied connecting to locald at {socket_path}")]
    #[diagnostic(
        code(locald::daemon::permission_denied),
        help("Check socket permissions or run `locald admin setup` (socket: {socket_path}).")
    )]
    PermissionDenied { socket_path: String },

    #[error("Failed to connect to locald at {socket_path}")]
    #[diagnostic(
        code(locald::daemon::connection_failed),
        help(
            "Check if the daemon is running and remove stale sockets (socket: {socket_path}, error: {source})."
        )
    )]
    ConnectionFailed {
        socket_path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("LOCALD_SOCKET cannot be used without LOCALD_SANDBOX_ACTIVE")]
    #[diagnostic(
        code(locald::daemon::socket_env_not_allowed),
        help("Unset LOCALD_SOCKET or set LOCALD_SANDBOX_ACTIVE=1 to use a sandbox socket.")
    )]
    SocketEnvNotAllowed,

    #[error("{message}")]
    #[diagnostic(
        code(locald::daemon::error),
        help("Check daemon logs at /tmp/locald.log (error: {message})")
    )]
    RequestFailed { message: String },
}

impl From<locald_utils::ipc::IpcError> for DaemonError {
    fn from(err: locald_utils::ipc::IpcError) -> Self {
        match err {
            locald_utils::ipc::IpcError::SocketEnvNotAllowed => Self::SocketEnvNotAllowed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miette::Diagnostic;

    #[test]
    fn daemon_error_not_running_has_correct_code() {
        let err = DaemonError::NotRunning {
            socket_path: "/tmp/test.sock".to_string(),
        };
        assert_eq!(
            err.code().unwrap().to_string(),
            "locald::daemon::not_running"
        );
    }

    #[test]
    fn daemon_error_connection_refused_has_correct_code() {
        let err = DaemonError::ConnectionRefused {
            socket_path: "/tmp/test.sock".to_string(),
        };
        assert_eq!(
            err.code().unwrap().to_string(),
            "locald::daemon::connection_refused"
        );
    }

    #[test]
    fn daemon_error_permission_denied_has_correct_code() {
        let err = DaemonError::PermissionDenied {
            socket_path: "/tmp/test.sock".to_string(),
        };
        assert_eq!(
            err.code().unwrap().to_string(),
            "locald::daemon::permission_denied"
        );
    }

    #[test]
    fn daemon_error_socket_env_not_allowed_has_correct_code() {
        let err = DaemonError::SocketEnvNotAllowed;
        assert_eq!(
            err.code().unwrap().to_string(),
            "locald::daemon::socket_env_not_allowed"
        );
    }

    #[test]
    fn daemon_error_request_failed_has_correct_code() {
        let err = DaemonError::RequestFailed {
            message: "test error".to_string(),
        };
        assert_eq!(err.code().unwrap().to_string(), "locald::daemon::error");
    }

    #[test]
    fn cli_error_message_creates_other_variant() {
        let err = CliError::message("test message");
        assert!(matches!(err, CliError::Other { .. }));
        assert_eq!(err.to_string(), "test message");
    }

    #[test]
    fn cli_error_from_anyhow_converts_to_other() {
        let anyhow_err = anyhow::anyhow!("anyhow error");
        let cli_err = CliError::from(anyhow_err);
        assert!(matches!(cli_err, CliError::Other { .. }));
    }

    #[test]
    fn daemon_error_from_ipc_error_converts_correctly() {
        let ipc_err = locald_utils::ipc::IpcError::SocketEnvNotAllowed;
        let daemon_err = DaemonError::from(ipc_err);
        assert!(matches!(daemon_err, DaemonError::SocketEnvNotAllowed));
    }

    #[test]
    fn daemon_errors_have_help_text() {
        let err = DaemonError::NotRunning {
            socket_path: "/tmp/test.sock".to_string(),
        };
        assert!(err.help().is_some());
        assert!(err.help().unwrap().to_string().contains("locald up"));
    }
}
