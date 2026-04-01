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
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
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
        menu.append(&PredefinedMenuItem::separator())?;

        let open_item = MenuItem::new("Open Dashboard", true, None);
        menu.append(&open_item)?;

        let quit_item = MenuItem::new("Quit", true, None);
        menu.append(&quit_item)?;

        let _tray_icon = TrayIconBuilder::new()
            .with_tooltip("locald")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .build()?;

        let (status_tx, status_rx) = mpsc::channel();

        spawn_status_thread(status_tx);

        // NSApplication.run() owns the event loop. We use an NSTimer to
        // periodically check our channels from the main thread.
        let menu_events = MenuEvent::receiver();
        let mut current_status = DaemonStatus::Checking;

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

            // Check for status updates from IPC thread.
            while let Ok(update) = status_rx.try_recv() {
                if update != current_status {
                    current_status = update;
                    status_item.set_text(current_status.label());
                }
            }

            // Check for menu item clicks.
            while let Ok(event) = menu_events.try_recv() {
                if event.id == open_item.id() {
                    open_dashboard();
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

    fn spawn_status_thread(status_tx: mpsc::Sender<DaemonStatus>) {
        thread::spawn(move || {
            loop {
                let status = poll_daemon_status();
                if status_tx.send(status).is_err() {
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

    fn send_request(request: &IpcRequest) -> Result<IpcResponse, String> {
        let socket_path = locald_utils::ipc::socket_path().map_err(|err| err.to_string())?;
        let mut stream = UnixStream::connect(&socket_path).map_err(|err| err.to_string())?;

        let request_bytes = serde_json::to_vec(request).map_err(|err| err.to_string())?;
        stream
            .write_all(&request_bytes)
            .map_err(|err| err.to_string())?;

        let mut response_bytes = Vec::new();
        stream
            .read_to_end(&mut response_bytes)
            .map_err(|err| err.to_string())?;

        serde_json::from_slice(&response_bytes).map_err(|err| err.to_string())
    }

    fn open_dashboard() {
        #[allow(clippy::disallowed_methods)]
        let _ = std::process::Command::new("open")
            .arg(DASHBOARD_URL)
            .spawn();
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
