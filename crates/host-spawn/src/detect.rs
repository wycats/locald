//! Container environment detection.

use std::path::Path;

use tokio::process::Command;

/// Detected container environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerEnvironment {
    /// Fedora Toolbx (uses flatpak-spawn).
    Toolbx,
    /// Distrobox container.
    Distrobox,
    /// Flatpak sandbox.
    Flatpak,
    /// Generic Docker/Podman (no host-exec).
    Docker,
    /// WSL2 on Windows.
    Wsl2,
    /// Not in a container.
    Native,
}

use crate::HostExec;

/// Detect the current container environment.
///
/// Uses a tiered detection hierarchy based on specification stability:
///
/// - **Tier 1 (Official Standards)**: `$FLATPAK_ID`, `/.flatpak-info`, `/run/systemd/container`
/// - **Tier 2 (De Facto Standards)**: `/run/.containerenv`, `/.dockerenv`
/// - **Tier 3 (Implementation Details)**: `$TOOLBOX_PATH`, `$DISTROBOX_ENTER_PATH`
pub async fn detect_container() -> ContainerEnvironment {
    // Tier 1: Official standards (most reliable)

    // Flatpak - officially documented env var
    if std::env::var("FLATPAK_ID").is_ok() {
        return ContainerEnvironment::Flatpak;
    }

    // Also check the official Flatpak marker file
    if Path::new("/.flatpak-info").exists() {
        return ContainerEnvironment::Flatpak;
    }

    // systemd Container Interface: user-readable file (preferred over env var
    // because $container is only set on PID 1, not inherited)
    if let Ok(manager) = tokio::fs::read_to_string("/run/systemd/container").await {
        let manager = manager.trim();
        // Map known manager names to our enum
        return match manager {
            "toolbox" => ContainerEnvironment::Toolbx,
            // Unknown but containerized - fall through to native detection
            _ => ContainerEnvironment::Docker,
        };
    }

    // WSL2 - Microsoft-defined
    if std::env::var("WSL_DISTRO_NAME").is_ok() {
        return ContainerEnvironment::Wsl2;
    }

    // Tier 2: De facto standards
    if Path::new("/run/.containerenv").exists() {
        // Could parse this file for more info (engine, name, rootless flag)
        return ContainerEnvironment::Docker;
    }
    if Path::new("/.dockerenv").exists() {
        return ContainerEnvironment::Docker;
    }

    // Tier 3: Implementation-specific env vars (undocumented but stable in practice)
    if std::env::var("TOOLBOX_PATH").is_ok() {
        return ContainerEnvironment::Toolbx;
    }
    if std::env::var("DISTROBOX_ENTER_PATH").is_ok() {
        return ContainerEnvironment::Distrobox;
    }

    ContainerEnvironment::Native
}

/// Auto-detect the best host-exec mechanism for the current environment.
///
/// Returns `None` if running natively (no container) or if no mechanism is available.
pub async fn detect_host_exec() -> Option<HostExec> {
    match detect_container().await {
        ContainerEnvironment::Toolbx | ContainerEnvironment::Flatpak => {
            if command_exists("flatpak-spawn").await {
                return Some(HostExec::FlatpakSpawn);
            }
        }
        ContainerEnvironment::Distrobox => {
            if command_exists("distrobox-host-exec").await {
                return Some(HostExec::DistroboxHostExec);
            }
        }
        ContainerEnvironment::Native => {
            return Some(HostExec::Direct);
        }
        ContainerEnvironment::Docker | ContainerEnvironment::Wsl2 => {
            // These environments don't have standard host-exec mechanisms
        }
    }

    // Fallback: try available mechanisms
    if command_exists("flatpak-spawn").await {
        return Some(HostExec::FlatpakSpawn);
    }
    if command_exists("distrobox-host-exec").await {
        return Some(HostExec::DistroboxHostExec);
    }

    None
}

/// Check if running in any container environment.
pub async fn is_containerized() -> bool {
    detect_container().await != ContainerEnvironment::Native
}

/// Check if a command exists on PATH.
pub async fn command_exists(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .await
        .is_ok_and(|o| o.status.success())
}
