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
    use locald_core::state::ServiceState;
    use locald_core::{IpcRequest, IpcResponse};
    use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::MainThreadMarker;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use tray_icon::{Icon, TrayIconBuilder};

    const DASHBOARD_URL: &str = "http://locald.localhost";

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum DaemonStatus {
        Checking,
        NotRunning,
        Running { total: usize, running: usize },
    }

    impl DaemonStatus {
        fn label(&self) -> String {
            match self {
                DaemonStatus::Checking => "Status: checking...".to_string(),
                DaemonStatus::NotRunning => "Status: not running".to_string(),
                DaemonStatus::Running { total, running } => {
                    if *total == 0 {
                        "Status: running (no services)".to_string()
                    } else {
                        format!("Status: {running}/{total} running")
                    }
                }
            }
        }
    }

    /// Health checks that run without root — detect problems the agent can surface.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct HealthStatus {
        pfctl_active: bool,
        pfctl_persistent: bool,
        ca_trusted: bool,
    }

    impl HealthStatus {
        fn is_healthy(&self) -> bool {
            self.pfctl_active && self.pfctl_persistent && self.ca_trusted
        }

        fn warning_label(&self) -> Option<String> {
            let mut problems = Vec::new();
            if !self.pfctl_active {
                problems.push("port forwarding inactive");
            } else if !self.pfctl_persistent {
                problems.push("port forwarding won't survive reboot");
            }
            if !self.ca_trusted {
                problems.push("HTTPS not trusted");
            }
            if problems.is_empty() {
                None
            } else {
                Some(format!("⚠ {}", problems.join(", ")))
            }
        }
    }

    /// Combined update from the polling thread.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct PollUpdate {
        daemon: DaemonStatus,
        health: HealthStatus,
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        // Initialize NSApplication — required for AppKit event dispatch (menu clicks, etc).
        let mtm = MainThreadMarker::new().expect("locald-agent must run on the main thread");
        let app = NSApplication::sharedApplication(mtm);
        // Accessory = no Dock icon, no app menu, just the menu bar item.
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let icon = build_icon()?;

        let menu = Menu::new();
        let status_item = MenuItem::new("Status: checking...", false, None);
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

        // NSApplication.run() owns the event loop. We use an NSTimer to
        // periodically check our channels from the main thread.
        let menu_events = MenuEvent::receiver();
        let mut current_daemon = DaemonStatus::Checking;
        let mut current_health = HealthStatus {
            pfctl_active: true,
            pfctl_persistent: true,
            ca_trusted: true,
        };

        // Call finishLaunching to initialize the app — registers with the
        // window server so click events are delivered.
        app.finishLaunching();

        loop {
            // Drain all pending AppKit events.
            loop {
                #[allow(unsafe_code)]
                let event = unsafe {
                    app.nextEventMatchingMask_untilDate_inMode_dequeue(
                        objc2_app_kit::NSEventMask::Any,
                        None, // don't wait — return immediately if no events
                        objc2_foundation::NSDefaultRunLoopMode,
                        true,
                    )
                };
                match event {
                    Some(event) => app.sendEvent(&event),
                    None => break,
                }
            }

            // Check for poll updates.
            while let Ok(update) = update_rx.try_recv() {
                if update.daemon != current_daemon {
                    current_daemon = update.daemon.clone();
                    status_item.set_text(current_daemon.label());
                }
                if update.health != current_health {
                    current_health = update.health.clone();
                    if let Some(label) = current_health.warning_label() {
                        health_item.set_text(label);
                        health_item.set_enabled(false);
                        setup_item.set_enabled(true);
                    } else {
                        health_item.set_text("");
                        health_item.set_enabled(false);
                        setup_item.set_enabled(false);
                    }
                }
            }

            // Check for menu item clicks.
            while let Ok(event) = menu_events.try_recv() {
                if event.id == open_item.id() {
                    open_dashboard();
                } else if event.id == restart_item.id() {
                    restart_all_services();
                } else if event.id == setup_item.id() && !current_health.is_healthy() {
                    run_admin_setup();
                } else if event.id == quit_item.id() {
                    return Ok(());
                }
            }

            // Wait for next event with a timeout. This blocks the thread
            // efficiently instead of busy-spinning, waking only when an AppKit
            // event arrives or the timeout expires.
            #[allow(unsafe_code)]
            let next = unsafe {
                app.nextEventMatchingMask_untilDate_inMode_dequeue(
                    objc2_app_kit::NSEventMask::Any,
                    Some(&objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.5)),
                    objc2_foundation::NSDefaultRunLoopMode,
                    true,
                )
            };
            if let Some(event) = next {
                app.sendEvent(&event);
            }
        }
    }

    fn spawn_poll_thread(update_tx: mpsc::Sender<PollUpdate>) {
        thread::spawn(move || {
            loop {
                let daemon = poll_daemon_status();
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
            Ok(IpcResponse::Status(services)) => {
                let running = services
                    .iter()
                    .filter(|service| service.status == ServiceState::Running)
                    .count();
                DaemonStatus::Running {
                    total: services.len(),
                    running,
                }
            }
            _ => DaemonStatus::NotRunning,
        }
    }

    /// Check system health without root.
    fn poll_health() -> HealthStatus {
        let pfctl_active = check_pfctl_active();
        HealthStatus {
            pfctl_active,
            pfctl_persistent: locald_utils::port_forward::is_persistent(),
            ca_trusted: locald_utils::cert::is_ca_trusted(),
        }
    }

    /// Probe whether pfctl port forwarding is active by attempting a TCP
    /// connection to localhost:80. If pfctl redirects 80→8080 and the daemon
    /// is listening on 8080, the connection succeeds. No root needed.
    ///
    /// Returns true if a TCP connection to port 80 succeeds (strong signal
    /// that pfctl rules are active), false on any error including timeouts.
    fn check_pfctl_active() -> bool {
        use std::net::{SocketAddr, TcpStream};

        let addr = SocketAddr::from(([127, 0, 0, 1], 80));
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
        let locald_path = locald_path_for_setup();
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

    /// Resolve the locald binary path for running admin setup.
    ///
    /// Prefers `locald` on PATH if it exists, otherwise falls back to a
    /// known install location.
    #[allow(clippy::disallowed_methods)]
    fn locald_path_for_setup() -> String {
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
        fn health_all_good() {
            let h = HealthStatus {
                pfctl_active: true,
                pfctl_persistent: true,
                ca_trusted: true,
            };
            assert!(h.is_healthy());
            assert_eq!(h.warning_label(), None);
        }

        #[test]
        fn health_pfctl_inactive() {
            let h = HealthStatus {
                pfctl_active: false,
                pfctl_persistent: false,
                ca_trusted: true,
            };
            assert!(!h.is_healthy());
            let label = h.warning_label().unwrap();
            assert!(label.contains("port forwarding inactive"));
            // When pfctl is inactive, don't also warn about persistence.
            assert!(!label.contains("reboot"));
        }

        #[test]
        fn health_pfctl_not_persistent() {
            let h = HealthStatus {
                pfctl_active: true,
                pfctl_persistent: false,
                ca_trusted: true,
            };
            assert!(!h.is_healthy());
            let label = h.warning_label().unwrap();
            assert!(label.contains("reboot"));
        }

        #[test]
        fn health_ca_untrusted() {
            let h = HealthStatus {
                pfctl_active: true,
                pfctl_persistent: true,
                ca_trusted: false,
            };
            assert!(!h.is_healthy());
            let label = h.warning_label().unwrap();
            assert!(label.contains("HTTPS not trusted"));
        }

        #[test]
        fn health_multiple_problems() {
            let h = HealthStatus {
                pfctl_active: false,
                pfctl_persistent: false,
                ca_trusted: false,
            };
            assert!(!h.is_healthy());
            let label = h.warning_label().unwrap();
            assert!(label.contains("port forwarding inactive"));
            assert!(label.contains("HTTPS not trusted"));
        }

        #[test]
        fn daemon_status_labels() {
            assert_eq!(DaemonStatus::Checking.label(), "Status: checking...");
            assert_eq!(DaemonStatus::NotRunning.label(), "Status: not running");
            assert_eq!(
                DaemonStatus::Running {
                    total: 3,
                    running: 2
                }
                .label(),
                "Status: 2/3 running"
            );
            assert_eq!(
                DaemonStatus::Running {
                    total: 0,
                    running: 0
                }
                .label(),
                "Status: running (no services)"
            );
        }

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
    }
}
