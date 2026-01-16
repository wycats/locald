//! Container environment detection and host execution helpers.
//!
//! This module provides utilities for:
//! - Detecting if we're running inside a container (Toolbx, Distrobox, Docker, etc.)
//! - Executing commands on the host from within a container
//! - Auto-starting the host shim daemon
//!
//! This module re-exports types from the `host-spawn` crate and provides
//! locald-specific wrappers for starting the host shim daemon.
//!
//! # Async vs Blocking
//!
//! The primary API is async. For early-startup code that runs before the tokio
//! runtime is available, use the [`blocking`] submodule which provides sync wrappers.

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

// Re-export host-spawn types for consumers
pub use host_spawn::{
    ContainerEnvironment, HostCommand, HostExec, HostSpawnError, Privilege, SpawnHost,
    command_exists, detect_container, detect_host_exec, is_containerized,
};

use crate::shim::SHIM_SERVE_SUDO;

/// Check if we're probably running inside a container.
///
/// This is a convenience wrapper around [`host_spawn::is_containerized`].
pub async fn is_probably_container() -> bool {
    is_containerized().await
}

/// Configuration for container host execution.
#[derive(Debug, Clone, Default)]
pub struct ContainerConfig {
    /// Optional user-configured `host_exec` template.
    /// If set, this template is used instead of auto-detection.
    /// Use `{command}` as placeholder for the command to run.
    pub host_exec: Option<String>,
}

/// Attempt to start the host shim daemon from within a container.
///
/// This tries various host-exec mechanisms in order:
/// 1. User-configured template from `ContainerConfig`
/// 2. flatpak-spawn (Toolbx/Flatpak containers)
/// 3. distrobox-host-exec (Distrobox containers)
///
/// The command starts the daemon in background mode (not --foreground).
///
/// # Errors
///
/// Returns an error if no host-exec mechanism is available or if all
/// available mechanisms fail to start the daemon.
pub async fn start_host_shim(config: &ContainerConfig) -> Result<()> {
    // Build the shim command
    let cmd = build_shim_command();

    // Determine host-exec mechanism
    let host_exec = if let Some(template) = &config.host_exec {
        HostExec::template(template.clone())
            .context("Invalid host_exec template: missing {command} placeholder")?
    } else {
        detect_host_exec().await.ok_or_else(|| {
            anyhow::anyhow!(
                "No host-exec mechanism available. \
                 Tried flatpak-spawn and distrobox-host-exec but neither is available.\n\n\
                 You can manually start the daemon on the host with:\n  \
                 {SHIM_SERVE_SUDO}\n\n\
                 Or configure a custom host_exec template in ~/.config/locald/config.toml:\n  \
                 [container]\n  \
                 host_exec = \"your-command {{command}}\""
            )
        })?
    };

    info!("Starting host shim: {:?}", host_exec.command_line(&cmd));

    let status = host_exec
        .spawn(&cmd)
        .await
        .context("Failed to execute host-exec mechanism")?;

    if status.success() {
        Ok(())
    } else {
        warn!("Host shim command failed with status: {status}");
        anyhow::bail!(
            "Host shim command failed with exit code {:?}.\n\n\
             You can manually start the daemon on the host with:\n  \
             {SHIM_SERVE_SUDO}",
            status.code()
        )
    }
}

/// Build the `HostCommand` for starting the shim daemon.
fn build_shim_command() -> HostCommand {
    // In shared-home containers (Toolbx, Distrobox), the shim binary built
    // alongside locald is visible from the host. Use that path if available.
    if let Ok(Some(shim_path)) = crate::shim::find() {
        // Found sibling shim - use absolute path (works because home is shared)
        let abs_path = shim_path.display().to_string();
        debug!("Using sibling shim at: {abs_path}");
        HostCommand::builder()
            .program(abs_path)
            .args(vec!["serve".into()])
            .privilege(Privilege::Pkexec)
            .build()
    } else {
        // Fall back to assuming locald-shim is in PATH on host
        debug!("Using locald-shim from PATH");
        HostCommand::builder()
            .program("locald-shim")
            .args(vec!["serve".into()])
            .privilege(Privilege::Pkexec)
            .build()
    }
}

/// Blocking (synchronous) versions of container functions.
///
/// Use these when calling from non-async code, such as early startup
/// before the tokio runtime is initialized. These functions use
/// `tokio::runtime::Runtime::block_on` internally.
pub mod blocking {
    use super::{ContainerConfig, Context, Result};

    /// Check if we're probably running inside a container (blocking version).
    ///
    /// This creates a temporary tokio runtime to execute the async detection.
    /// Prefer the async version when possible.
    #[allow(clippy::disallowed_methods)] // Intentionally blocking for early startup
    pub fn is_probably_container() -> bool {
        // For container detection, we can use a simpler sync approach
        // that doesn't require the full tokio runtime for most checks.

        // Tier 1: Environment variables (instant, no I/O)
        if std::env::var("FLATPAK_ID").is_ok() {
            return true;
        }
        if std::env::var("WSL_DISTRO_NAME").is_ok() {
            return true;
        }
        if std::env::var("TOOLBOX_PATH").is_ok() {
            return true;
        }
        if std::env::var("DISTROBOX_ENTER_PATH").is_ok() {
            return true;
        }
        if std::env::var("container").is_ok() {
            return true;
        }

        // Tier 2: Marker files
        std::path::Path::new("/.flatpak-info").exists()
            || std::path::Path::new("/run/.containerenv").exists()
            || std::path::Path::new("/.dockerenv").exists()
            || std::path::Path::new("/run/systemd/container").exists()
    }

    /// Start the host shim daemon (blocking version).
    ///
    /// This creates a temporary tokio runtime to execute the async spawn.
    /// Prefer the async version when possible.
    ///
    /// # Errors
    ///
    /// Returns an error if no host-exec mechanism is available or if the
    /// host shim command fails to execute.
    #[allow(clippy::disallowed_methods)] // Intentionally blocking for early startup
    pub fn start_host_shim(config: &ContainerConfig) -> Result<()> {
        // Create a current-thread runtime for the blocking call
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("Failed to create tokio runtime for blocking start_host_shim")?;

        rt.block_on(super::start_host_shim(config))
    }

    /// Check if a command exists on PATH (blocking version).
    #[allow(clippy::disallowed_methods)] // Intentionally blocking for early startup
    pub fn command_exists(name: &str) -> bool {
        std::process::Command::new("which")
            .arg(name)
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// Run a command on the host from within a container (blocking version).
    ///
    /// This auto-detects the available host-exec mechanism (flatpak-spawn or
    /// distrobox-host-exec) and runs the provided command with arguments.
    ///
    /// # Arguments
    ///
    /// * `args` - The command and arguments to run on the host
    ///
    /// # Errors
    ///
    /// Returns an error if no host-exec mechanism is available or if the
    /// command fails to execute.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use locald_utils::container::blocking::run_on_host;
    ///
    /// let status = run_on_host(&["sudo", "--", "locald", "admin", "setup"])?;
    /// if status.success() {
    ///     println!("Setup completed!");
    /// }
    /// ```
    #[allow(clippy::disallowed_methods)] // Intentionally blocking for early startup
    pub fn run_on_host(args: &[&str]) -> Result<std::process::ExitStatus> {
        // Detect available host-exec mechanism
        let host_exec = if command_exists("flatpak-spawn") {
            Some(("flatpak-spawn", &["--host"] as &[&str]))
        } else if command_exists("distrobox-host-exec") {
            Some(("distrobox-host-exec", &[] as &[&str]))
        } else {
            None
        };

        let Some((exec_cmd, exec_args)) = host_exec else {
            anyhow::bail!(
                "No host-exec mechanism available. \
                 Tried flatpak-spawn and distrobox-host-exec but neither is available."
            );
        };

        std::process::Command::new(exec_cmd)
            .args(exec_args)
            .args(args)
            .status()
            .with_context(|| format!("Failed to execute {exec_cmd}"))
    }
}
