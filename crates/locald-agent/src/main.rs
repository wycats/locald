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

        // "Run Setup..." — visible only when health checks fail.
        let setup_item = MenuItem::new("Run Setup...", true, None);
        menu.append(&setup_item)?;

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

    fn open_dashboard() {
        #[allow(clippy::disallowed_methods)]
        let _ = std::process::Command::new("open")
            .arg(DASHBOARD_URL)
            .spawn();
    }

    /// Launch `locald admin setup` in Terminal.app via osascript.
    ///
    /// This opens a new Terminal window where the command auto-escalates
    /// to root (prompts for sudo password in the terminal).
    /// The resolved path is shell-quoted to handle spaces and special characters.
    fn run_admin_setup() {
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
}
