use crate::error::{CliError, CliResult};
use crate::style;
use anyhow::Context;
use crossterm::style::Stylize;
use locald_core::IpcRequest;

#[allow(unsafe_code)]
pub fn setup_sandbox(name: &str) -> CliResult<()> {
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

pub fn spawn_daemon() -> CliResult<()> {
    let exe_path = std::env::current_exe()?;

    // Do not try to auto-repair the privileged shim here.
    // - The daemon can run without privileged ports (e.g. LOCALD_HTTP_PORT=0 in tests).
    // - Daemon contexts must never block on interactive sudo prompts.
    // Privileged operations (port binding, container execution) enforce shim requirements at call sites.

    let log_file = std::fs::File::create("/tmp/locald.log")?;

    // Use pre_exec to call setsid() (the POSIX syscall) in the child process before
    // exec. This creates a new session so the daemon isn't killed by SIGHUP when the
    // terminal closes. Works on both Linux and macOS.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        let mut cmd = std::process::Command::new(&exe_path);
        cmd.arg("server")
            .arg("start")
            .stdout(log_file.try_clone()?)
            .stderr(log_file);
        // SAFETY: setsid() is async-signal-safe (POSIX.1-2008). It only affects the
        // child process between fork and exec, with no side effects in the parent.
        #[allow(unsafe_code)]
        unsafe {
            cmd.pre_exec(|| {
                nix::unistd::setsid().map_err(std::io::Error::other)?;
                Ok(())
            });
        }
        cmd.spawn()?;
    }

    #[cfg(not(unix))]
    {
        std::process::Command::new(&exe_path)
            .arg("server")
            .arg("start")
            .stdout(log_file.try_clone()?)
            .stderr(log_file)
            .spawn()?;
    }

    std::thread::sleep(std::time::Duration::from_millis(500));
    Ok(())
}

pub fn ensure_daemon_running() -> CliResult<()> {
    let sandbox_active = std::env::var("LOCALD_SANDBOX_ACTIVE").is_ok();
    // Try to ping first
    match crate::client::send_request(&IpcRequest::Ping) {
        Ok(_) => return Ok(()),
        Err(e) => {
            if !sandbox_active {
                if let Ok(path) = locald_utils::ipc::socket_path() {
                    eprintln!("Ping failed on {}: {}", path.display(), e);
                } else {
                    eprintln!("Ping failed: {}", e);
                }
            }
        }
    }

    if !sandbox_active {
        println!("Starting locald server...");
    }
    spawn_daemon()?;

    // Wait for it to be ready
    for _ in 0..50 {
        if crate::client::send_request(&IpcRequest::Ping).is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    Err(CliError::message("Timed out waiting for locald to start"))
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

    let status = std::process::Command::new("sudo")
        .arg(exe)
        .arg("admin")
        .arg("setup")
        .status();

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

// Re-export blocking container functions from locald-utils for use in early-startup code
#[cfg(target_os = "linux")]
pub use locald_utils::container::blocking::is_probably_container;

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
const fn is_probably_container() -> bool {
    false
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

    let setup_prompt = if use_pkexec {
        "Run `pkexec locald admin setup` now? (GUI auth dialog)"
    } else {
        "Run `sudo locald admin setup` now?"
    };

    let run_setup = Confirm::new()
        .with_prompt(setup_prompt)
        .default(true)
        .interact()
        .unwrap_or(false);

    if !run_setup {
        return false;
    }

    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to get executable path: {}", e);
            std::process::exit(1);
        }
    };

    let status = if use_pkexec {
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
}

/// Show error that locald cannot run inside containers.
#[cfg(target_os = "linux")]
fn show_container_unsupported_error() -> ! {
    eprintln!();
    eprintln!(
        "{} locald does not support running inside containers.",
        style::CROSS
    );
    eprintln!();
    eprintln!("Please run locald on the host OS.");
    eprintln!();
    eprintln!(
        "If you need the CLI inside a container, expose the host binary into the container using your tooling."
    );
    eprintln!();
    std::process::exit(1);
}

#[allow(clippy::missing_const_for_fn)]
pub fn verify_shim() {
    #[cfg(target_os = "macos")]
    {
        use dialoguer::Confirm;
        use std::io::IsTerminal;

        // Skip setup verification in sandbox mode (used for testing)
        if std::env::var("LOCALD_SANDBOX_ACTIVE").is_ok() {
            return;
        }

        let config_path = crate::global_config::global_config_path();
        let config_file_exists = config_path
            .as_ref()
            .map(|path| path.exists())
            .unwrap_or(false);
        if !config_file_exists {
            return;
        }

        let config = crate::global_config::load();
        if !config.server.privileged_ports {
            return;
        }

        let certs_dir = match locald_utils::cert::get_certs_dir() {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!(
                    "{} Failed to determine locald certs directory: {}",
                    style::CROSS,
                    e
                );
                std::process::exit(1);
            }
        };

        let ca_path = certs_dir.join("rootCA.pem");
        if ca_path.exists() {
            return;
        }

        if !std::io::stdin().is_terminal() {
            eprintln!(
                "{} locald needs initial setup. Run `sudo locald admin setup`.",
                style::CROSS
            );
            std::process::exit(1);
        }

        let run_setup = Confirm::new()
            .with_prompt("locald needs initial setup. Run it now?")
            .default(true)
            .interact()
            .unwrap_or(false);

        if !run_setup {
            eprintln!(
                "{} locald needs initial setup. Run `sudo locald admin setup`.",
                style::CROSS
            );
            std::process::exit(1);
        }

        let exe_path = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to get executable path: {}", e);
                std::process::exit(1);
            }
        };

        let status = std::process::Command::new("sudo")
            .arg("--")
            .arg(&exe_path)
            .arg("admin")
            .arg("setup")
            .status();

        match status {
            Ok(s) if s.success() => {
                eprintln!();
                eprintln!(
                    "{} Setup complete! Continuing with your command...",
                    style::CHECK
                );
                eprintln!();
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
    }

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
            if is_probably_container() {
                show_container_unsupported_error();
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
    #[allow(unused_imports)]
    use super::*;

    // The following tests document the container detection behavior for locald.

    /// Documents the container detection heuristics.
    /// This test validates the logic without actually checking filesystem markers.
    #[test]
    #[cfg(target_os = "linux")]
    fn container_detection_heuristics_are_documented() {
        // The function checks these signals in order:
        // 1. $container env var (set by container runtimes)
        // 2. Runtime-specific env vars used by some container runtimes
        // 3. /run/.containerenv (common container marker file)
        // 4. /.dockerenv (common container marker file)

        // We can't easily test filesystem markers in unit tests, but we can
        // verify the function exists and is callable.
        // The actual integration testing happens via locald-e2e.
        let _ = is_probably_container();
    }

    /// Container detection remains for friendly errors, but locald is host-only.
    #[test]
    #[cfg(target_os = "linux")]
    fn container_detection_still_exists_for_guidance() {
        let _ = is_probably_container();
    }
}
