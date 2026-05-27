mod status;

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    macos::run()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("locald-agent is macOS-only");
}

#[cfg(target_os = "macos")]
mod macos {
    use crate::status::{DaemonStatus, HealthStatus, PollUpdate};
    use locald_core::{IpcRequest, IpcResponse};
    use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::MainThreadMarker;
    use std::cell::RefCell;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use tray_icon::{Icon, TrayIconBuilder};

    const DASHBOARD_URL: &str = "http://locald.localhost";
    const LOCALD_DAEMON_PATH_ENV: &str = "LOCALD_DAEMON_PATH";

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        // Initialize NSApplication — required for AppKit event dispatch (menu clicks, etc).
        let mtm = MainThreadMarker::new().expect("locald-agent must run on the main thread");
        let app = NSApplication::sharedApplication(mtm);
        // Accessory = no Dock icon, no app menu, just the menu bar item.
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let icon = build_icon()?;

        let menu = Menu::new();
        let status_item = MenuItem::new(DaemonStatus::Checking.label(), false, None);
        menu.append(&status_item)?;

        // Health warning — hidden when everything is healthy.
        let health_item = MenuItem::new("", false, None);
        menu.append(&health_item)?;

        menu.append(&PredefinedMenuItem::separator())?;

        let open_item = MenuItem::new("Open Dashboard", true, None);
        menu.append(&open_item)?;

        let restart_item = MenuItem::new("Restart All Services", true, None);
        menu.append(&restart_item)?;

        // "Run Setup..." — visible only when health checks fail.
        let setup_item = MenuItem::new("Run Setup...", true, None);
        menu.append(&setup_item)?;

        menu.append(&PredefinedMenuItem::separator())?;

        let quit_item = MenuItem::new("Quit", true, None);
        menu.append(&quit_item)?;

        // Initially hide health-related items.
        health_item.set_text("");
        health_item.set_enabled(false);
        setup_item.set_enabled(false);

        let _tray_icon = TrayIconBuilder::new()
            .with_tooltip("locald")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .build()?;

        let (update_tx, update_rx) = mpsc::channel();

        spawn_poll_thread(update_tx);

        // Use NSApplication::run() for the event loop. This properly handles
        // all system events including display reconfiguration (monitor switching),
        // sleep/wake, and menu bar rebuilds. A manual nextEvent pump misses some
        // of these, causing the tray icon to disappear on display changes.
        //
        // We check our channels (status updates, menu clicks) via an NSTimer
        // callback that fires every 0.5 seconds on the main run loop.
        let menu_events = MenuEvent::receiver();

        // State shared with the timer callback via thread-local RefCell
        // (the callback runs on the main thread, same as NSApplication::run).
        thread_local! {
            static STATE: RefCell<Option<CallbackState>> = const { RefCell::new(None) };
        }

        struct CallbackState {
            update_rx: mpsc::Receiver<PollUpdate>,
            current_daemon: DaemonStatus,
            current_health: HealthStatus,
            status_item: MenuItem,
            health_item: MenuItem,
            setup_item: MenuItem,
            open_item_id: muda::MenuId,
            restart_item_id: muda::MenuId,
            setup_item_id: muda::MenuId,
            quit_item_id: muda::MenuId,
        }

        STATE.with(|s| {
            *s.borrow_mut() = Some(CallbackState {
                update_rx,
                current_daemon: DaemonStatus::Checking,
                current_health: HealthStatus {
                    helper_installed: true,
                    port_80_reachable: true,
                    ca_trusted: true,
                },
                status_item,
                health_item,
                setup_item: setup_item.clone(),
                open_item_id: open_item.id().clone(),
                restart_item_id: restart_item.id().clone(),
                setup_item_id: setup_item.id().clone(),
                quit_item_id: quit_item.id().clone(),
            });
        });

        // Schedule a repeating timer on the main run loop.
        let timer_callback = block2::RcBlock::new(
            move |_timer: std::ptr::NonNull<objc2_foundation::NSTimer>| {
                STATE.with(|cell| {
                    let mut borrow = cell.borrow_mut();
                    let Some(state) = borrow.as_mut() else {
                        return;
                    };

                    // Check for poll updates.
                    while let Ok(update) = state.update_rx.try_recv() {
                        if update.daemon != state.current_daemon {
                            state.current_daemon = update.daemon.clone();
                            state.status_item.set_text(state.current_daemon.label());
                        }
                        if update.health != state.current_health {
                            state.current_health = update.health.clone();
                            if let Some(label) = state.current_health.warning_label() {
                                state.health_item.set_text(label);
                                state.health_item.set_enabled(false);
                                state.setup_item.set_enabled(true);
                            } else {
                                state.health_item.set_text("");
                                state.health_item.set_enabled(false);
                                state.setup_item.set_enabled(false);
                            }
                        }
                    }

                    // Check for menu item clicks.
                    while let Ok(event) = menu_events.try_recv() {
                        if event.id == state.open_item_id {
                            open_dashboard();
                        } else if event.id == state.restart_item_id {
                            restart_all_services();
                        } else if event.id == state.setup_item_id
                            && !state.current_health.is_healthy()
                        {
                            run_admin_setup();
                        } else if event.id == state.quit_item_id {
                            let app =
                                NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
                            app.terminate(None);
                        }
                    }
                });
            },
        );

        #[allow(unsafe_code)]
        unsafe {
            use objc2_foundation::NSTimer;
            let _timer =
                NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.5, true, &timer_callback);
        }

        // NSApplication::run() never returns. It properly handles all system
        // events including display reconfiguration.
        app.run();

        // run() never returns, but Rust needs a return value.
        #[allow(unreachable_code)]
        Ok(())
    }

    fn spawn_poll_thread(update_tx: mpsc::Sender<PollUpdate>) {
        thread::spawn(move || {
            let mut last_start_attempt: Option<std::time::Instant> = None;
            loop {
                let daemon = poll_daemon_status();

                if daemon == DaemonStatus::NotRunning {
                    let should_start =
                        last_start_attempt.is_none_or(|t| t.elapsed() >= Duration::from_secs(30));
                    if should_start {
                        last_start_attempt = Some(std::time::Instant::now());
                        start_daemon();
                    }
                }

                let health = poll_health();
                if update_tx.send(PollUpdate { daemon, health }).is_err() {
                    break;
                }
                #[allow(clippy::disallowed_methods)]
                thread::sleep(Duration::from_secs(3));
            }
        });
    }

    fn poll_daemon_status() -> DaemonStatus {
        if !matches!(send_request(&IpcRequest::Ping), Ok(IpcResponse::Pong)) {
            return DaemonStatus::NotRunning;
        }

        match send_request(&IpcRequest::Status) {
            Ok(IpcResponse::Status(services)) => DaemonStatus::from_services(&services),
            _ => DaemonStatus::NotRunning,
        }
    }

    /// Check system health without root.
    fn poll_health() -> HealthStatus {
        let helper_installed =
            std::path::Path::new("/Library/PrivilegedHelperTools/com.locald.helper").exists();
        let port_80_reachable = check_port_reachable(80);
        HealthStatus {
            helper_installed,
            port_80_reachable,
            ca_trusted: locald_utils::cert::is_ca_trusted(),
        }
    }

    /// Probe whether port 80 is reachable by attempting a TCP connection.
    /// When the helper has bound port 80 and passed the FD to the server,
    /// this connection succeeds.
    fn check_port_reachable(port: u16) -> bool {
        use std::net::{SocketAddr, TcpStream};

        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
    }

    fn send_request(request: &IpcRequest) -> Result<IpcResponse, Box<dyn std::error::Error>> {
        let request_bytes = serde_json::to_vec(request)?;
        let response_bytes = locald_utils::ipc::send_request(&request_bytes)?;
        Ok(serde_json::from_slice(&response_bytes)?)
    }

    fn restart_all_services() {
        let _ = send_request(&IpcRequest::RestartAll);
    }

    fn open_dashboard() {
        #[allow(clippy::disallowed_methods)]
        let _ = std::process::Command::new("open")
            .arg(DASHBOARD_URL)
            .spawn();
    }

    /// Attempt privileged setup, preferring XPC helper, falling back to Terminal.
    fn run_admin_setup() {
        if try_xpc_setup() {
            return;
        }
        run_admin_setup_via_terminal();
    }

    /// Try to perform setup via the privileged XPC helper.
    ///
    /// Connects to the `com.locald.helper` Mach service and sends a setup
    /// command. Returns true if the helper responded with success.
    fn try_xpc_setup() -> bool {
        use std::sync::mpsc;

        // XPC types aren't Send, so we run the entire XPC exchange on a
        // dedicated thread and communicate the result back via a channel.
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let result = xpc_setup_on_thread();
            let _ = tx.send(result);
        });

        // Wait up to 30s for the helper to respond.
        rx.recv_timeout(Duration::from_secs(30)).unwrap_or(false)
    }

    /// Perform the XPC setup exchange. Must run on the thread that created
    /// the `XpcClient` (XPC types are not `Send`).
    fn xpc_setup_on_thread() -> bool {
        use futures::stream::StreamExt;
        use std::ffi::CString;
        use xpc_connection::{Message, XpcClient};

        #[allow(clippy::expect_used)]
        let name = CString::new("com.locald.helper").expect("static CString");
        let mut client = XpcClient::connect(&name);

        // Build { "command": "setup" } dictionary.
        let mut dict = std::collections::HashMap::new();
        #[allow(clippy::expect_used)]
        {
            dict.insert(
                CString::new("command").expect("static CString"),
                Message::String(CString::new("setup").expect("static CString")),
            );
        }
        client.send_message(Message::Dictionary(dict));

        // Drive the future to completion on a single-threaded runtime.
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return false;
        };

        let result = rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(25), client.next())
                .await
                .ok()
                .flatten()
        });

        match result {
            Some(Message::Dictionary(ref dict)) => {
                #[allow(clippy::expect_used)]
                let status_key = CString::new("status").expect("static CString");
                #[allow(clippy::expect_used)]
                let success_val = CString::new("success").expect("static CString");
                matches!(dict.get(&status_key), Some(Message::String(s)) if *s == success_val)
            }
            _ => false,
        }
    }

    /// Launch `locald admin setup` in Terminal.app via osascript.
    ///
    /// This opens a new Terminal window where the command auto-escalates
    /// to root (prompts for sudo password in the terminal).
    /// The resolved path is shell-quoted to handle spaces and special characters.
    fn run_admin_setup_via_terminal() {
        let locald_path = locald_path_for_admin_setup();
        let command = format!("{} admin setup", shell_quote(&locald_path));
        // Escape for embedding inside an AppleScript double-quoted string.
        let escaped = command.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!("tell application \"Terminal\" to do script \"{escaped}\"");
        #[allow(clippy::disallowed_methods)]
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn();
    }

    /// POSIX single-quote a string for safe shell interpolation.
    fn shell_quote(s: &str) -> String {
        if s.is_empty() {
            return "''".to_string();
        }
        let mut out = String::with_capacity(s.len() + 2);
        out.push('\'');
        for ch in s.chars() {
            if ch == '\'' {
                out.push_str("'\\''");
            } else {
                out.push(ch);
            }
        }
        out.push('\'');
        out
    }

    /// Spawn the daemon if it isn't running. Errors are intentionally
    /// swallowed — the poll loop will retry on the next cycle.
    fn start_daemon() {
        let exe_path = match locald_path_for_daemon_start() {
            Ok(path) => path,
            Err(message) => {
                log_agent_message(&format!("locald-agent: {message}"));
                return;
            }
        };
        let log_file = match std::fs::File::create("/tmp/locald.log") {
            Ok(f) => f,
            Err(_) => return,
        };
        let log_clone = match log_file.try_clone() {
            Ok(f) => f,
            Err(_) => return,
        };

        use std::os::unix::process::CommandExt;
        #[allow(clippy::disallowed_methods)]
        let mut cmd = std::process::Command::new(&exe_path);
        cmd.arg("server")
            .arg("start")
            .stdout(log_clone)
            .stderr(log_file);
        #[allow(unsafe_code)]
        unsafe {
            cmd.pre_exec(|| {
                nix::unistd::setsid().map_err(std::io::Error::other)?;
                Ok(())
            });
        }
        if let Err(e) = cmd.spawn() {
            log_agent_message(&format!(
                "locald-agent: failed to start daemon from {exe_path}: {e}"
            ));
        }
    }

    /// Resolve the locald binary path for running admin setup.
    ///
    /// Prefers `locald` on PATH if it exists, otherwise falls back to a
    /// known install location.
    #[allow(clippy::disallowed_methods)]
    fn locald_path_for_admin_setup() -> String {
        // Check if locald is on PATH.
        if let Ok(output) = std::process::Command::new("which").arg("locald").output()
            && output.status.success()
        {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return path;
            }
        }
        // Fallback: assume standard install location.
        "/usr/local/bin/locald".to_string()
    }

    /// Resolve the pinned locald binary path for daemon auto-start.
    ///
    /// The LaunchAgent should provide this. If it does not, the agent skips
    /// daemon auto-start rather than guessing from PATH.
    fn locald_path_for_daemon_start() -> Result<String, String> {
        match std::env::var(LOCALD_DAEMON_PATH_ENV) {
            Ok(value) => locald_path_for_daemon_start_from_value(Some(&value)),
            Err(std::env::VarError::NotPresent) => Err(format!(
                "{LOCALD_DAEMON_PATH_ENV} is not set; skipping daemon auto-start"
            )),
            Err(std::env::VarError::NotUnicode(_)) => Err(format!(
                "{LOCALD_DAEMON_PATH_ENV} is not valid UTF-8; skipping daemon auto-start"
            )),
        }
    }

    fn locald_path_for_daemon_start_from_value(value: Option<&str>) -> Result<String, String> {
        match value {
            Some(path) if !path.trim().is_empty() => Ok(path.trim().to_string()),
            Some(_) => Err(format!(
                "{LOCALD_DAEMON_PATH_ENV} is empty; skipping daemon auto-start"
            )),
            None => Err(format!(
                "{LOCALD_DAEMON_PATH_ENV} is not set; skipping daemon auto-start"
            )),
        }
    }

    fn log_agent_message(message: &str) {
        use std::io::Write;

        if let Ok(mut log_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/locald.log")
        {
            let _ = writeln!(log_file, "{message}");
        }
    }

    fn build_icon() -> Result<Icon, Box<dyn std::error::Error>> {
        // 44x44 for Retina displays (macOS menu bar is ~22pt, @2x = 44px).
        let size: usize = 44;
        let radius = 8.0_f32;
        let center = (size as f32 - 1.0) / 2.0;
        let mut rgba = Vec::with_capacity(size * size * 4);

        for y in 0..size {
            for x in 0..size {
                let dx = x as f32 - center;
                let dy = y as f32 - center;
                let distance = (dx * dx + dy * dy).sqrt();
                // Anti-alias the edge for a crisp circle.
                let alpha = (radius - distance + 0.5).clamp(0.0, 1.0);
                let a = (alpha * 255.0) as u8;
                // Use a teal that works on both light and dark menu bars.
                rgba.extend_from_slice(&[0x1e, 0x9d, 0xd9, a]);
            }
        }

        Ok(Icon::from_rgba(rgba, size as u32, size as u32)?)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn shell_quote_simple_path() {
            assert_eq!(
                shell_quote("/usr/local/bin/locald"),
                "'/usr/local/bin/locald'"
            );
        }

        #[test]
        fn shell_quote_path_with_spaces() {
            assert_eq!(
                shell_quote("/Applications/My App/locald"),
                "'/Applications/My App/locald'"
            );
        }

        #[test]
        fn shell_quote_path_with_single_quote() {
            assert_eq!(shell_quote("it's"), "'it'\\''s'");
        }

        #[test]
        fn shell_quote_empty() {
            assert_eq!(shell_quote(""), "''");
        }

        #[test]
        fn daemon_start_path_uses_pinned_env_value() {
            assert_eq!(
                locald_path_for_daemon_start_from_value(Some("/opt/locald/bin/locald")),
                Ok("/opt/locald/bin/locald".to_string())
            );
        }

        #[test]
        fn daemon_start_path_trims_pinned_env_value() {
            assert_eq!(
                locald_path_for_daemon_start_from_value(Some("  /opt/locald/bin/locald  ")),
                Ok("/opt/locald/bin/locald".to_string())
            );
        }

        #[test]
        fn daemon_start_path_rejects_missing_env_value() {
            assert_eq!(
                locald_path_for_daemon_start_from_value(None),
                Err("LOCALD_DAEMON_PATH is not set; skipping daemon auto-start".to_string())
            );
        }

        #[test]
        fn daemon_start_path_rejects_empty_env_value() {
            assert_eq!(
                locald_path_for_daemon_start_from_value(Some("  ")),
                Err("LOCALD_DAEMON_PATH is empty; skipping daemon auto-start".to_string())
            );
        }
    }
}
