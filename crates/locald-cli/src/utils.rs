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

pub fn verify_shim() {
    #[cfg(target_os = "linux")]
    {
        // Only check if we are NOT already running under the shim
        if std::env::var("LOCALD_SHIM_ACTIVE").is_err() {
            if let Ok(Some(shim_path)) = locald_utils::shim::find_privileged() {
                #[cfg(debug_assertions)]
                {
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
                            // If we can't verify, it's safer to exit.
                            std::process::exit(1);
                        }
                    }
                }

                #[cfg(not(debug_assertions))]
                {
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
                            eprintln!("{} Failed to verify shim: {}", style::CROSS, e);
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
    }
}
