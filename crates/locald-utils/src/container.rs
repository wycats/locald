//! Container environment detection and host execution helpers.
//!
//! This module provides utilities for:
//! - Detecting if we're running inside a container (Toolbx, Distrobox, Docker, etc.)
//! - Executing commands on the host from within a container
//! - Auto-starting the host shim daemon

// This module intentionally uses synchronous I/O since it runs during
// privilege acquisition before the async runtime is established.
#![allow(clippy::disallowed_methods)]

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use tracing::{info, warn};

use crate::shim::{SHIM_SERVE_PKEXEC, SHIM_SERVE_SUDO};

/// Check if we're probably running inside a container.
///
/// This is a heuristic check that looks for common container indicators.
pub fn is_probably_container() -> bool {
    // Check environment variables
    if std::env::var("container").is_ok()
        || std::env::var("TOOLBOX_PATH").is_ok()
        || std::env::var("DISTROBOX_ENTER_PATH").is_ok()
    {
        return true;
    }

    // Check for container marker files
    Path::new("/run/.containerenv").exists() || Path::new("/.dockerenv").exists()
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
pub fn start_host_shim(config: &ContainerConfig) -> Result<()> {
    let shim_command = SHIM_SERVE_PKEXEC;

    // Try user-configured template first
    if let Some(template) = &config.host_exec {
        let full_command = substitute_template(template, shim_command);
        info!("Starting host shim using configured template: {full_command}");
        return run_shell_command(&full_command);
    }

    // Auto-detect: try flatpak-spawn first (Toolbx)
    if command_exists("flatpak-spawn") {
        info!("Starting host shim via flatpak-spawn");
        let status = Command::new("flatpak-spawn")
            .arg("--host")
            .args(shim_command.split_whitespace())
            .status()
            .context("Failed to execute flatpak-spawn")?;

        if status.success() {
            return Ok(());
        }
        warn!("flatpak-spawn failed with status: {status}");
    }

    // Try distrobox-host-exec
    if command_exists("distrobox-host-exec") {
        info!("Starting host shim via distrobox-host-exec");
        let status = Command::new("distrobox-host-exec")
            .args(shim_command.split_whitespace())
            .status()
            .context("Failed to execute distrobox-host-exec")?;

        if status.success() {
            return Ok(());
        }
        warn!("distrobox-host-exec failed with status: {status}");
    }

    anyhow::bail!(
        "No host-exec mechanism available. \
         Tried flatpak-spawn and distrobox-host-exec but neither succeeded.\n\n\
         You can manually start the daemon on the host with:\n  \
         {SHIM_SERVE_SUDO}\n\n\
         Or configure a custom host_exec template in ~/.config/locald/config.toml:\n  \
         [container]\n  \
         host_exec = \"your-command {{command}}\""
    )
}

/// Substitute `{command}` placeholder in a template.
#[allow(clippy::literal_string_with_formatting_args)]
fn substitute_template(template: &str, command: &str) -> String {
    template.replace("{command}", command)
}

/// Check if a command exists on PATH.
fn command_exists(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run a shell command (for template execution).
fn run_shell_command(cmd: &str) -> Result<()> {
    let status = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .status()
        .with_context(|| format!("Failed to execute: {cmd}"))?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("Command failed with status: {status}")
    }
}
