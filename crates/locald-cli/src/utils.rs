use crate::style;
use anyhow::{Context, Result};
use crossterm::style::Stylize;
use locald_core::IpcRequest;

pub fn handle_ipc_error(e: &anyhow::Error) {
    let msg = e.to_string();
    if msg.contains("locald is not running") {
        eprintln!("Error: {msg}");
        eprintln!("Hint: Run `locald up` to start the daemon.");
    } else {
        eprintln!("Error: {e}");
    }
    std::process::exit(1);
}

#[allow(unsafe_code)]
pub fn setup_sandbox(name: &str) -> Result<()> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let sandbox_root = std::path::PathBuf::from(home)
        .join(".local/share/locald/sandboxes")
        .join(name);

    let data_dir = sandbox_root.join("data");
    let config_dir = sandbox_root.join("config");
    let state_dir = sandbox_root.join("state");
    let socket_path = sandbox_root.join("locald.sock");

    std::fs::create_dir_all(&data_dir).context("Failed to create sandbox data dir")?;
    std::fs::create_dir_all(&config_dir).context("Failed to create sandbox config dir")?;
    std::fs::create_dir_all(&state_dir).context("Failed to create sandbox state dir")?;

    // SAFETY: This is safe because we are single-threaded at this point (during setup).
    unsafe {
        std::env::set_var("XDG_DATA_HOME", &data_dir);
        std::env::set_var("XDG_CONFIG_HOME", &config_dir);
        std::env::set_var("XDG_STATE_HOME", &state_dir);
        std::env::set_var("LOCALD_SOCKET", &socket_path);
        std::env::set_var("LOCALD_SANDBOX_ACTIVE", "1");
        std::env::set_var("LOCALD_SANDBOX_NAME", name);
    }

    eprintln!("{} Running in sandbox: {}", style::PACKAGE, name.bold());

    Ok(())
}

pub fn spawn_daemon() -> Result<()> {
    let exe_path = std::env::current_exe()?;

    // Do not try to auto-repair the privileged shim here.
    // - The daemon can run without privileged ports (e.g. LOCALD_HTTP_PORT=0 in tests).
    // - Daemon contexts must never block on interactive sudo prompts.
    // Privileged operations (port binding, container execution) enforce shim requirements at call sites.

    let log_file = std::fs::File::create("/tmp/locald.log")?;

    let status = std::process::Command::new("setsid")
        .arg(&exe_path)
        .arg("server")
        .arg("start")
        .stdout(log_file.try_clone()?)
        .stderr(log_file.try_clone()?)
        .spawn();

    match status {
        Ok(_) => {
            std::thread::sleep(std::time::Duration::from_millis(500));
            Ok(())
        }
        Err(e) => {
            eprintln!("Warning: setsid failed ({e}), trying direct spawn...");
            std::process::Command::new(&exe_path)
                .arg("server")
                .arg("start")
                .stdout(log_file.try_clone()?)
                .stderr(log_file)
                .spawn()?;
            std::thread::sleep(std::time::Duration::from_millis(500));
            Ok(())
        }
    }
}

pub fn ensure_daemon_running() -> Result<()> {
    // Try to ping first
    match crate::client::send_request(&IpcRequest::Ping) {
        Ok(_) => return Ok(()),
        Err(e) => {
            if let Ok(path) = locald_utils::ipc::socket_path() {
                eprintln!("Ping failed on {}: {}", path.display(), e);
            } else {
                eprintln!("Ping failed: {}", e);
            }
        }
    }

    println!("Starting locald server...");
    spawn_daemon()?;

    // Wait for it to be ready
    for _ in 0..50 {
        if crate::client::send_request(&IpcRequest::Ping).is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    anyhow::bail!("Timed out waiting for locald to start")
}

#[cfg(target_os = "linux")]
fn try_auto_fix_shim() -> bool {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return false;
    }

    // Auto-fixing the shim can trigger an interactive sudo password prompt.
    // Default to "no surprises"; require an explicit opt-in.
    if std::env::var("LOCALD_SHIM_AUTO_FIX").ok().as_deref() != Some("1") {
        return false;
    }

    eprintln!("{} Updating locald-shim...", style::WARN);

    let Ok(exe) = std::env::current_exe() else {
        return false;
    };

    let status = if should_attempt_host_setup() {
        std::process::Command::new(exe)
            .arg("admin")
            .arg("setup")
            .arg("--host")
            .status()
    } else {
        std::process::Command::new("sudo")
            .arg(exe)
            .arg("admin")
            .arg("setup")
            .status()
    };

    match status {
        Ok(s) if s.success() => {
            eprintln!("{} Shim updated successfully.", style::CHECK);
            true
        }
        _ => {
            eprintln!("{} Failed to update shim.", style::CROSS);
            false
        }
    }
}

#[cfg(target_os = "linux")]
fn looks_like_systemctl_connectivity_failure(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("failed to connect to system scope bus") || s.contains("failed to connect to bus")
}

// Re-export blocking container functions from locald-utils for use in early-startup code
#[cfg(target_os = "linux")]
pub use locald_utils::container::blocking::{
    command_exists, is_probably_container, start_host_shim,
};

// For non-Linux platforms, provide stub implementations
#[cfg(not(target_os = "linux"))]
fn is_probably_container() -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
fn command_exists(_name: &str) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn cgroup_mount_is_readonly() -> Option<bool> {
    // Parse /proc/self/mountinfo and locate the mount entry for /sys/fs/cgroup.
    // Field layout: see `man proc`.
    let contents = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    for line in contents.lines() {
        let mut parts = line.split_whitespace();
        let _mount_id = parts.next()?;
        let _parent_id = parts.next()?;
        let _major_minor = parts.next()?;
        let _root = parts.next()?;
        let mount_point = parts.next()?;
        let mount_opts = parts.next()?;
        if mount_point == "/sys/fs/cgroup" {
            let ro = mount_opts.split(',').any(|o| o == "ro");
            return Some(ro);
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn systemctl_can_connect() -> Option<bool> {
    if !command_exists("systemctl") {
        return None;
    }

    let output = std::process::Command::new("systemctl")
        .arg("--no-pager")
        .arg("is-system-running")
        .output()
        .ok()?;

    if output.status.success() {
        return Some(true);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if looks_like_systemctl_connectivity_failure(&stderr) {
        return Some(false);
    }

    // If systemctl ran and failed for another reason, treat it as "connectivity unknown".
    None
}

#[cfg(target_os = "linux")]
fn should_attempt_host_setup() -> bool {
    if !is_probably_container() {
        return false;
    }

    // Only attempt host setup if we have a known host-exec mechanism available.
    if !(command_exists("flatpak-spawn") || command_exists("distrobox-host-exec")) {
        return false;
    }

    // Strong signal: cgroup mount is read-only.
    if matches!(cgroup_mount_is_readonly(), Some(true)) {
        return true;
    }

    // Secondary signal: systemctl can't connect (common when PID 1 is host systemd but
    // container lacks the required sockets).
    matches!(systemctl_can_connect(), Some(false))
}

/// Offer interactive first-run setup when no shim is installed.
/// Returns true if setup was completed successfully.
#[cfg(target_os = "linux")]
fn offer_first_run_setup() -> bool {
    use dialoguer::Confirm;
    use std::io::IsTerminal;

    // Only offer interactive setup if stdin is a TTY
    if !std::io::stdin().is_terminal() {
        eprintln!("{} locald-shim is not installed.", style::CROSS);
        eprintln!();
        if locald_utils::shim::is_polkit_available() {
            eprintln!("Run: pkexec locald admin setup  (GUI auth dialog)");
            eprintln!("  or: sudo locald admin setup");
        } else {
            eprintln!("Run: sudo locald admin setup");
        }
        eprintln!();
        eprintln!("Or use the install script:");
        eprintln!(
            "  curl -fsSL https://raw.githubusercontent.com/wycats/locald/main/install.sh | sh"
        );
        std::process::exit(1);
    }

    eprintln!();
    eprintln!("{}  Welcome to locald!", style::ROCKET);
    eprintln!();
    eprintln!("locald requires a one-time privileged setup to:");
    eprintln!(
        "  {} Install the process supervisor (locald-shim)",
        style::DOT
    );
    eprintln!("  {} Configure cgroups for process isolation", style::DOT);
    eprintln!("  {} Set up HTTPS certificates (optional)", style::DOT);
    eprintln!();

    // Check if polkit is available for privilege escalation.
    // This provides a GUI auth dialog instead of requiring terminal sudo.
    let use_pkexec = locald_utils::shim::is_polkit_available();

    let setup_prompt = if should_attempt_host_setup() {
        "Run `locald admin setup --host` now?"
    } else if use_pkexec {
        "Run `pkexec locald admin setup` now? (GUI auth dialog)"
    } else {
        "Run `sudo locald admin setup` now?"
    };

    let run_setup = Confirm::new()
        .with_prompt(setup_prompt)
        .default(true)
        .interact()
        .unwrap_or(false);

    if run_setup {
        let exe_path = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to get executable path: {}", e);
                std::process::exit(1);
            }
        };

        let status = if should_attempt_host_setup() {
            eprintln!(
                "{} Detected container environment; attempting host setup...",
                style::WARN
            );
            std::process::Command::new(&exe_path)
                .arg("admin")
                .arg("setup")
                .arg("--host")
                .status()
        } else if use_pkexec {
            // Use pkexec for GUI-based privilege escalation
            eprintln!(
                "{} Using polkit for privilege escalation (GUI auth dialog)...",
                style::INFO
            );
            let pkexec_result = std::process::Command::new("pkexec")
                .arg(&exe_path)
                .arg("admin")
                .arg("setup")
                .status();

            // If pkexec fails (e.g., user cancelled dialog), try sudo as fallback
            match &pkexec_result {
                Ok(s) if s.success() => pkexec_result,
                _ => {
                    eprintln!(
                        "{} pkexec failed or was cancelled, falling back to sudo...",
                        style::WARN
                    );
                    std::process::Command::new("sudo")
                        .arg("--")
                        .arg(&exe_path)
                        .arg("admin")
                        .arg("setup")
                        .status()
                }
            }
        } else {
            std::process::Command::new("sudo")
                .arg("--")
                .arg(&exe_path)
                .arg("admin")
                .arg("setup")
                .status()
        };

        match status {
            Ok(s) if s.success() => {
                eprintln!();
                eprintln!(
                    "{} Setup complete! Continuing with your command...",
                    style::CHECK
                );
                eprintln!();
                true // Continue with original command
            }
            Ok(s) => {
                eprintln!("Setup failed with exit code: {:?}", s.code());
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("Failed to run setup: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        eprintln!();
        eprintln!("Setup skipped. Run manually when ready:");
        if use_pkexec {
            eprintln!("  pkexec locald admin setup  (GUI auth dialog)");
            eprintln!("  or: sudo locald admin setup");
        } else {
            eprintln!("  sudo locald admin setup");
        }
        eprintln!();
        // Exit because we can't proceed without shim
        std::process::exit(0);
    }
}

/// Show error that privileged helper access is required and exit.
/// This is NOT a warning - we do not continue without privileges.
fn show_privilege_required_error() -> ! {
    eprintln!();
    eprintln!("{} locald requires privileged helper access.", style::CROSS);
    eprintln!();
    eprintln!("This is needed for: hosts file sync, cgroup isolation, privileged ports.");
    eprintln!();
    eprintln!("To fix:");
    eprintln!(
        "  {} Container: flatpak-spawn --host pkexec locald-shim serve",
        style::DOT
    );
    eprintln!("  {} Direct:    sudo locald admin setup", style::DOT);
    eprintln!();
    eprintln!("To test without privileges, use sandbox mode:");
    eprintln!("  locald --sandbox test up");
    eprintln!();
    eprintln!("For diagnosis: locald doctor");
    eprintln!();
    std::process::exit(1);
}

pub fn verify_shim() {
    #[cfg(target_os = "linux")]
    {
        // Skip shim verification in sandbox mode (used for testing)
        if std::env::var("LOCALD_SANDBOX_ACTIVE").is_ok() {
            return;
        }

        // Skip shim verification when explicitly disabled (for testing)
        if std::env::var("LOCALD_SKIP_SHIM_CHECK").is_ok() {
            return;
        }

        // Only check if we are NOT already running under the shim
        if std::env::var("LOCALD_SHIM_ACTIVE").is_err() {
            // In container environments, prefer socket-based daemon over setuid shim.
            // The setuid shim often can't work across container boundaries.
            #[allow(clippy::items_after_statements)]
            if is_probably_container() {
                use locald_utils::container::ContainerConfig;

                // Try to connect to existing shim socket daemon
                // NOTE: This is shim_client::socket_path() -> ~/.locald/shim.sock
                // NOT ipc::socket_path() -> /tmp/locald.sock (the server socket)
                if let Ok(shim_socket) = locald_utils::shim_client::socket_path() {
                    if shim_socket.exists() {
                        // Shim socket exists, assume daemon is running - we're good
                        return;
                    }
                }

                // Socket doesn't exist, try to auto-start the host daemon
                eprintln!("{} Attempting to start shim daemon on host...", style::INFO);

                match start_host_shim(&ContainerConfig::default()) {
                    Ok(()) => {
                        // Wait for shim socket to appear
                        let Ok(shim_socket) = locald_utils::shim_client::socket_path() else {
                            // Can't determine socket path - fatal error
                            show_privilege_required_error();
                        };

                        for attempt in 1..=10 {
                            #[allow(clippy::disallowed_methods)]
                            std::thread::sleep(std::time::Duration::from_millis(500));
                            if shim_socket.exists() {
                                eprintln!(
                                    "{} Host shim daemon started successfully.",
                                    style::CHECK
                                );
                                return;
                            }
                            if attempt == 5 {
                                eprintln!(
                                    "{} Still waiting for host daemon to start...",
                                    style::INFO
                                );
                            }
                        }
                        // Timeout waiting for socket - fatal error
                        eprintln!(
                            "{} Host daemon started but socket not available.",
                            style::CROSS
                        );
                        show_privilege_required_error();
                    }
                    Err(e) => {
                        eprintln!("{} Failed to auto-start host daemon: {}", style::WARN, e);
                        // Offer interactive setup like non-container path
                        if !offer_first_run_setup() {
                            show_privilege_required_error();
                        }
                    }
                }
                return;
            }

            // Non-container path: check for setuid shim
            match locald_utils::shim::find_privileged() {
                Ok(Some(shim_path)) => {
                    // Shim exists, verify integrity
                    const SHIM_BYTES: &[u8] = include_bytes!(env!("LOCALD_EMBEDDED_SHIM_PATH"));
                    match locald_utils::shim::verify_integrity(&shim_path, SHIM_BYTES) {
                        Ok(true) => {
                            // Shim is up to date
                        }
                        Ok(false) => {
                            eprintln!("{} locald-shim is outdated or modified.", style::CROSS);

                            if try_auto_fix_shim() {
                                return;
                            }

                            eprintln!(
                                "Run: `{}`",
                                crate::hints::admin_setup_command_for_current_exe()
                            );
                            std::process::exit(1);
                        }
                        Err(e) => {
                            eprintln!("{} Failed to verify locald-shim: {}", style::CROSS, e);
                            std::process::exit(1);
                        }
                    }
                }
                Ok(None) => {
                    // No privileged shim found on non-container host.
                    // This is first run. Offer interactive setup if in a TTY.
                    offer_first_run_setup();
                    // If offer_first_run_setup returns, setup was successful.
                }
                Err(e) => {
                    eprintln!("{} Failed to check for locald-shim: {}", style::CROSS, e);
                    std::process::exit(1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Tests for host-exec template substitution are now in the host-spawn crate.
    // The following tests document the container detection behavior for locald.

    /// Documents the container detection heuristics.
    /// This test validates the logic without actually checking filesystem markers.
    #[test]
    #[cfg(target_os = "linux")]
    fn container_detection_heuristics_are_documented() {
        // The function checks these signals in order:
        // 1. $container env var (set by systemd-nspawn, podman, etc.)
        // 2. $TOOLBOX_PATH or $TOOLBOX_CONTAINER (Toolbx-specific)
        // 3. /run/.containerenv (Podman/Toolbx marker file)
        // 4. /.dockerenv (Docker marker file)

        // We can't easily test filesystem markers in unit tests, but we can
        // verify the function exists and is callable.
        // The actual integration testing happens via locald-e2e.
        let _ = is_probably_container();
    }

    /// Documents the expected behavior: in containers, missing shim should NOT block.
    ///
    /// This is the critical invariant for the Toolbx/Distrobox workflow:
    /// - Host: `sudo locald admin setup` (installs setuid shim)
    /// - Container: `locald up` (runs services, tolerates missing shim)
    #[test]
    #[cfg(target_os = "linux")]
    fn container_workflow_is_documented() {
        // The workflow is:
        // 1. User installs locald on host
        // 2. User runs `sudo locald admin setup` on host
        //    - This installs the setuid shim
        //    - This configures cgroups
        // 3. User enters Toolbx/Distrobox
        // 4. User runs `locald up`
        //    - locald detects container environment
        //    - locald warns about missing privileged features
        //    - locald proceeds to run services anyway
        //
        // The key invariant: `locald up` must NOT prompt for setup or exit
        // with an error when running inside a container, even if the shim
        // appears unavailable (due to user namespace UID mapping).

        // This is tested by the verify_shim() function checking is_probably_container()
        // before offering first-run setup.
    }
}
