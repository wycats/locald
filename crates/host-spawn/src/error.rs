//! Error types for host-spawn operations.

use thiserror::Error;

/// Errors that can occur during host command execution.
#[derive(Debug, Error)]
pub enum HostSpawnError {
    /// No mechanism available to execute commands on the host.
    #[error("no host-exec mechanism available (tried flatpak-spawn, distrobox-host-exec)")]
    NoHostExecMechanism,

    /// The host-exec mechanism exists but failed to execute.
    #[error("host-exec mechanism '{mechanism}' is not available")]
    MechanismUnavailable {
        /// The name of the mechanism that was unavailable.
        mechanism: &'static str,
    },

    /// I/O error during command execution.
    #[error("failed to spawn command: {0}")]
    Spawn(#[source] std::io::Error),

    /// Command executed but returned non-zero exit code.
    #[error("command failed with exit code {code:?}")]
    NonZeroExit {
        /// The exit code, if available.
        code: Option<i32>,
    },

    /// Template substitution error.
    #[error("invalid template: missing {{command}} placeholder")]
    InvalidTemplate,

    /// Argument contains characters that cannot be safely escaped for shell.
    #[error("argument contains unescapable characters: {arg:?}")]
    CommandEscapingFailed {
        /// The problematic argument.
        arg: String,
    },
}

/// Convenience type alias for Results with [`HostSpawnError`].
pub type Result<T> = std::result::Result<T, HostSpawnError>;
