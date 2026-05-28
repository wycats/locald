#![cfg_attr(not(test), allow(dead_code))]

#[cfg(target_os = "linux")]
use crate::status::{DaemonStatus, HealthStatus, PollUpdate};
#[cfg(target_os = "linux")]
use locald_core::{IpcRequest, IpcResponse};

use std::fmt;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
const DASHBOARD_URL: &str = "http://locald.localhost";
#[cfg(target_os = "linux")]
const LOCALD_DAEMON_PATH_ENV: &str = "LOCALD_DAEMON_PATH";

/// Snapshot of Linux desktop session state that matters before attempting to
/// register a StatusNotifierItem.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct HostEnvironment {
    pub(crate) wayland_display: Option<String>,
    pub(crate) x11_display: Option<String>,
    pub(crate) dbus_session_bus: Option<String>,
    pub(crate) current_desktop: Option<String>,
    pub(crate) desktop_session: Option<String>,
}

impl HostEnvironment {
    pub(crate) fn from_env() -> Self {
        Self {
            wayland_display: non_empty_env("WAYLAND_DISPLAY"),
            x11_display: non_empty_env("DISPLAY"),
            dbus_session_bus: non_empty_env("DBUS_SESSION_BUS_ADDRESS"),
            current_desktop: non_empty_env("XDG_CURRENT_DESKTOP"),
            desktop_session: non_empty_env("DESKTOP_SESSION"),
        }
    }

    pub(crate) fn has_graphical_session(&self) -> bool {
        self.wayland_display.is_some() || self.x11_display.is_some()
    }

    pub(crate) fn has_session_bus(&self) -> bool {
        self.dbus_session_bus.is_some()
    }

    pub(crate) fn is_gnome(&self) -> bool {
        self.current_desktop
            .as_deref()
            .is_some_and(desktop_names_include_gnome)
            || self
                .desktop_session
                .as_deref()
                .is_some_and(desktop_names_include_gnome)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusNotifierProbe {
    Available,
    WatcherMissing,
    HostWillNotShow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostDiagnostic {
    NoGraphicalSession,
    NoSessionBus,
    MissingStatusNotifier { desktop: DesktopKind },
    InvisibleStatusNotifierHost { desktop: DesktopKind },
}

impl HostDiagnostic {
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::NoGraphicalSession => {
                "Linux tray support requires a graphical desktop session. Start `locald tray` from a session with WAYLAND_DISPLAY or DISPLAY set, or use the locald CLI/dashboard instead."
            }
            Self::NoSessionBus => {
                "Linux tray support requires a D-Bus session bus. Start `locald tray` from a desktop session with DBUS_SESSION_BUS_ADDRESS set, or use the locald CLI/dashboard instead."
            }
            Self::MissingStatusNotifier {
                desktop: DesktopKind::Gnome,
            } => {
                "GNOME does not expose StatusNotifier/AppIndicator support in this session. Enable or install AppIndicator/StatusNotifier support for GNOME, then run `locald tray start` again; otherwise use the locald CLI/dashboard."
            }
            Self::MissingStatusNotifier {
                desktop: DesktopKind::Other,
            } => {
                "No StatusNotifier/AppIndicator watcher is available in this desktop session. Enable a tray host/status notifier plugin, then run `locald tray start` again; otherwise use the locald CLI/dashboard."
            }
            Self::InvisibleStatusNotifierHost {
                desktop: DesktopKind::Gnome,
            } => {
                "GNOME accepted the tray startup path, but no visible StatusNotifier/AppIndicator host is available. Enable or install GNOME AppIndicator/StatusNotifier support, then run `locald tray start` again."
            }
            Self::InvisibleStatusNotifierHost {
                desktop: DesktopKind::Other,
            } => {
                "The StatusNotifier item cannot be shown because no visible tray host is available. Enable a tray host/status notifier plugin, then run `locald tray start` again."
            }
        }
    }
}

impl fmt::Display for HostDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for HostDiagnostic {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DesktopKind {
    Gnome,
    Other,
}

pub(crate) fn diagnose_host(
    environment: &HostEnvironment,
    status_notifier_probe: StatusNotifierProbe,
) -> Result<(), HostDiagnostic> {
    if !environment.has_graphical_session() {
        return Err(HostDiagnostic::NoGraphicalSession);
    }

    if !environment.has_session_bus() {
        return Err(HostDiagnostic::NoSessionBus);
    }

    let desktop = if environment.is_gnome() {
        DesktopKind::Gnome
    } else {
        DesktopKind::Other
    };

    match status_notifier_probe {
        StatusNotifierProbe::Available => Ok(()),
        StatusNotifierProbe::WatcherMissing => {
            Err(HostDiagnostic::MissingStatusNotifier { desktop })
        }
        StatusNotifierProbe::HostWillNotShow => {
            Err(HostDiagnostic::InvisibleStatusNotifierHost { desktop })
        }
    }
}

#[cfg(target_os = "linux")]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(run_async())?;
    Ok(())
}

#[cfg(target_os = "linux")]
async fn run_async() -> Result<(), HostDiagnostic> {
    use ksni::TrayMethods;

    let environment = HostEnvironment::from_env();
    diagnose_host(&environment, StatusNotifierProbe::Available)?;

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let tray = LocaldTray::new(event_tx.clone());
    let handle = tray
        .spawn()
        .await
        .map_err(|error| host_diagnostic_from_ksni_error(&environment, &error))?;

    spawn_poll_thread(event_tx);

    while let Some(event) = event_rx.recv().await {
        match event {
            TrayEvent::Update(update) => {
                if handle
                    .update(|tray| {
                        tray.daemon = update.daemon;
                        tray.health = update.health;
                    })
                    .await
                    .is_none()
                {
                    break;
                }
            }
            TrayEvent::Action(TrayAction::OpenDashboard) => open_dashboard(),
            TrayEvent::Action(TrayAction::RestartAllServices) => restart_all_services(),
            TrayEvent::Action(TrayAction::RunSetup) => run_admin_setup(),
            TrayEvent::Action(TrayAction::Quit) => {
                handle.shutdown().await;
                break;
            }
        }
    }

    Ok(())
}

#[cfg(target_os = "linux")]
#[derive(Clone)]
struct LocaldTray {
    daemon: DaemonStatus,
    health: HealthStatus,
    event_tx: tokio::sync::mpsc::UnboundedSender<TrayEvent>,
}

#[cfg(target_os = "linux")]
impl LocaldTray {
    fn new(event_tx: tokio::sync::mpsc::UnboundedSender<TrayEvent>) -> Self {
        Self {
            daemon: DaemonStatus::Checking,
            health: HealthStatus {
                helper_installed: true,
                port_80_reachable: true,
                ca_trusted: true,
            },
            event_tx,
        }
    }
}

#[cfg(target_os = "linux")]
impl ksni::Tray for LocaldTray {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "locald".to_string()
    }

    fn title(&self) -> String {
        "locald".to_string()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::ApplicationStatus
    }

    fn status(&self) -> ksni::Status {
        if self.health.is_healthy() {
            ksni::Status::Active
        } else {
            ksni::Status::NeedsAttention
        }
    }

    fn icon_name(&self) -> String {
        "applications-development".to_string()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![build_icon()]
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let mut items = vec![disabled_item(self.daemon.label())];

        if let Some(label) = self.health.warning_label() {
            items.push(disabled_item(label));
        }

        items.push(ksni::MenuItem::Separator);
        items.push(action_item(
            "Open Dashboard",
            self.event_tx.clone(),
            TrayAction::OpenDashboard,
        ));
        items.push(action_item(
            "Restart All Services",
            self.event_tx.clone(),
            TrayAction::RestartAllServices,
        ));

        if !self.health.is_healthy() {
            items.push(action_item(
                "Run Setup...",
                self.event_tx.clone(),
                TrayAction::RunSetup,
            ));
        }

        items.push(ksni::MenuItem::Separator);
        items.push(action_item("Quit", self.event_tx.clone(), TrayAction::Quit));
        items
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        let diagnostic = match reason {
            ksni::OfflineReason::Error(error) => {
                host_diagnostic_from_ksni_error(&HostEnvironment::from_env(), &error)
            }
            ksni::OfflineReason::No => HostDiagnostic::InvisibleStatusNotifierHost {
                desktop: desktop_kind_from_environment(&HostEnvironment::from_env()),
            },
            _ => HostDiagnostic::InvisibleStatusNotifierHost {
                desktop: desktop_kind_from_environment(&HostEnvironment::from_env()),
            },
        };
        log_agent_message(&format!("locald-agent: {diagnostic}"));
        false
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayAction {
    OpenDashboard,
    RestartAllServices,
    RunSetup,
    Quit,
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
enum TrayEvent {
    Update(PollUpdate),
    Action(TrayAction),
}

#[cfg(target_os = "linux")]
fn disabled_item(label: String) -> ksni::MenuItem<LocaldTray> {
    ksni::menu::StandardItem {
        label,
        enabled: false,
        visible: true,
        activate: Box::new(|_tray: &mut LocaldTray| {}),
        ..Default::default()
    }
    .into()
}

#[cfg(target_os = "linux")]
fn action_item(
    label: &str,
    event_tx: tokio::sync::mpsc::UnboundedSender<TrayEvent>,
    action: TrayAction,
) -> ksni::MenuItem<LocaldTray> {
    ksni::menu::StandardItem {
        label: label.to_string(),
        enabled: true,
        visible: true,
        activate: Box::new(move |_tray: &mut LocaldTray| {
            let _ = event_tx.send(TrayEvent::Action(action));
        }),
        ..Default::default()
    }
    .into()
}

#[cfg(target_os = "linux")]
fn host_diagnostic_from_ksni_error(
    environment: &HostEnvironment,
    error: &ksni::Error,
) -> HostDiagnostic {
    let desktop = desktop_kind_from_environment(environment);
    match error {
        ksni::Error::Dbus(_) => HostDiagnostic::NoSessionBus,
        ksni::Error::Watcher(_) => HostDiagnostic::MissingStatusNotifier { desktop },
        ksni::Error::WontShow => HostDiagnostic::InvisibleStatusNotifierHost { desktop },
        _ => HostDiagnostic::MissingStatusNotifier { desktop },
    }
}

#[cfg(target_os = "linux")]
fn desktop_kind_from_environment(environment: &HostEnvironment) -> DesktopKind {
    if environment.is_gnome() {
        DesktopKind::Gnome
    } else {
        DesktopKind::Other
    }
}

#[cfg(target_os = "linux")]
fn spawn_poll_thread(event_tx: tokio::sync::mpsc::UnboundedSender<TrayEvent>) {
    std::thread::spawn(move || {
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
            if event_tx
                .send(TrayEvent::Update(PollUpdate { daemon, health }))
                .is_err()
            {
                break;
            }
            #[allow(clippy::disallowed_methods)]
            std::thread::sleep(Duration::from_secs(3));
        }
    });
}

#[cfg(target_os = "linux")]
fn poll_daemon_status() -> DaemonStatus {
    if !matches!(send_request(&IpcRequest::Ping), Ok(IpcResponse::Pong)) {
        return DaemonStatus::NotRunning;
    }

    match send_request(&IpcRequest::Status) {
        Ok(IpcResponse::Status(services)) => DaemonStatus::from_services(&services),
        _ => DaemonStatus::NotRunning,
    }
}

#[cfg(target_os = "linux")]
fn poll_health() -> HealthStatus {
    HealthStatus {
        helper_installed: locald_utils::shim::find_privileged()
            .ok()
            .flatten()
            .is_some(),
        port_80_reachable: check_port_reachable(80),
        ca_trusted: is_ca_trusted(),
    }
}

#[cfg(target_os = "linux")]
fn is_ca_trusted() -> bool {
    let Ok(certs_dir) = locald_utils::cert::get_certs_dir() else {
        return false;
    };
    let root_ca = certs_dir.join("rootCA.pem");
    if !root_ca.exists() {
        return false;
    }

    [
        "/usr/local/share/ca-certificates/locald-rootCA.crt",
        "/etc/pki/ca-trust/source/anchors/locald-rootCA.pem",
        "/etc/ca-certificates/trust-source/anchors/locald-rootCA.pem",
    ]
    .iter()
    .any(|path| std::path::Path::new(path).exists())
}

#[cfg(target_os = "linux")]
fn check_port_reachable(port: u16) -> bool {
    use std::net::{SocketAddr, TcpStream};

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

#[cfg(target_os = "linux")]
fn send_request(request: &IpcRequest) -> Result<IpcResponse, Box<dyn std::error::Error>> {
    let request_bytes = serde_json::to_vec(request)?;
    let response_bytes = locald_utils::ipc::send_request(&request_bytes)?;
    Ok(serde_json::from_slice(&response_bytes)?)
}

#[cfg(target_os = "linux")]
fn restart_all_services() {
    if let Err(error) = send_request(&IpcRequest::RestartAll) {
        log_agent_message(&format!(
            "locald-agent: failed to restart all services: {error}"
        ));
    }
}

#[cfg(target_os = "linux")]
#[allow(clippy::disallowed_methods)]
fn open_dashboard() {
    if let Err(error) = std::process::Command::new("xdg-open")
        .arg(DASHBOARD_URL)
        .spawn()
    {
        log_agent_message(&format!("locald-agent: failed to open dashboard: {error}"));
    }
}

#[cfg(target_os = "linux")]
fn run_admin_setup() {
    let locald_path = locald_path_for_admin_setup();
    let command = format!(
        "{} admin setup; printf '\\nPress Enter to close...'; read _",
        shell_quote(&locald_path)
    );

    if let Err(error) = spawn_terminal(&command) {
        log_agent_message(&format!(
            "locald-agent: failed to launch admin setup: {error}"
        ));
    }
}

#[cfg(target_os = "linux")]
#[allow(clippy::disallowed_methods)]
fn spawn_terminal(command: &str) -> std::io::Result<()> {
    let mut last_error = None;
    for candidate in [
        TerminalCommand::new("x-terminal-emulator", &["-e", "sh", "-lc"]),
        TerminalCommand::new("gnome-terminal", &["--", "sh", "-lc"]),
        TerminalCommand::new("konsole", &["-e", "sh", "-lc"]),
        TerminalCommand::new("xterm", &["-e", "sh", "-lc"]),
    ] {
        match candidate.spawn(command) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no terminal command configured",
        )
    }))
}

#[cfg(target_os = "linux")]
struct TerminalCommand<'a> {
    binary: &'a str,
    args: &'a [&'a str],
}

#[cfg(target_os = "linux")]
impl<'a> TerminalCommand<'a> {
    const fn new(binary: &'a str, args: &'a [&'a str]) -> Self {
        Self { binary, args }
    }

    #[allow(clippy::disallowed_methods)]
    fn spawn(&self, command: &str) -> std::io::Result<()> {
        std::process::Command::new(self.binary)
            .args(self.args)
            .arg(command)
            .spawn()
            .map(|_| ())
    }
}

#[cfg(target_os = "linux")]
#[allow(clippy::disallowed_methods)]
fn locald_path_for_admin_setup() -> String {
    if let Ok(path) = std::env::var(LOCALD_DAEMON_PATH_ENV)
        && !path.trim().is_empty()
    {
        return path;
    }

    if let Ok(output) = std::process::Command::new("which").arg("locald").output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return path;
        }
    }

    "/usr/local/bin/locald".to_string()
}

#[cfg(target_os = "linux")]
fn start_daemon() {
    let locald_path = locald_path_for_admin_setup();
    let log_file = match std::fs::File::create("/tmp/locald.log") {
        Ok(file) => file,
        Err(error) => {
            log_agent_message(&format!(
                "locald-agent: failed to create daemon log: {error}"
            ));
            return;
        }
    };
    let log_clone = match log_file.try_clone() {
        Ok(file) => file,
        Err(error) => {
            log_agent_message(&format!(
                "locald-agent: failed to clone daemon log handle: {error}"
            ));
            return;
        }
    };

    #[allow(clippy::disallowed_methods)]
    let mut cmd = std::process::Command::new(&locald_path);
    cmd.arg("server")
        .arg("start")
        .stdout(log_clone)
        .stderr(log_file);

    if let Err(error) = cmd.spawn() {
        log_agent_message(&format!(
            "locald-agent: failed to start daemon from {locald_path}: {error}"
        ));
    }
}

#[cfg(target_os = "linux")]
fn build_icon() -> ksni::Icon {
    let size: usize = 44;
    let radius = 8.0_f32;
    let center = (size as f32 - 1.0) / 2.0;
    let mut data = Vec::with_capacity(size * size * 4);

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            let alpha = (radius - distance + 0.5).clamp(0.0, 1.0);
            let a = (alpha * 255.0) as u8;
            data.extend_from_slice(&[a, 0x1e, 0x9d, 0xd9]);
        }
    }

    ksni::Icon {
        width: size as i32,
        height: size as i32,
        data,
    }
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn desktop_names_include_gnome(names: &str) -> bool {
    names
        .split([':', ';'])
        .any(|name| name.trim().eq_ignore_ascii_case("gnome"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop_environment() -> HostEnvironment {
        HostEnvironment {
            wayland_display: Some("wayland-0".to_string()),
            x11_display: None,
            dbus_session_bus: Some("unix:path=/run/user/1000/bus".to_string()),
            current_desktop: Some("GNOME".to_string()),
            desktop_session: None,
        }
    }

    #[test]
    fn graphical_session_accepts_wayland_or_x11() {
        let mut environment = HostEnvironment::default();
        assert!(!environment.has_graphical_session());

        environment.wayland_display = Some("wayland-0".to_string());
        assert!(environment.has_graphical_session());

        environment.wayland_display = None;
        environment.x11_display = Some(":0".to_string());
        assert!(environment.has_graphical_session());
    }

    #[test]
    fn detects_gnome_from_current_desktop_list() {
        let environment = HostEnvironment {
            current_desktop: Some("ubuntu:GNOME".to_string()),
            ..HostEnvironment::default()
        };

        assert!(environment.is_gnome());
    }

    #[test]
    fn can_snapshot_host_environment_from_process_env() {
        let _environment = HostEnvironment::from_env();
    }

    #[test]
    fn rejects_headless_sessions_before_status_notifier_probe() {
        let mut environment = desktop_environment();
        environment.wayland_display = None;
        environment.x11_display = None;

        assert_eq!(
            diagnose_host(&environment, StatusNotifierProbe::Available),
            Err(HostDiagnostic::NoGraphicalSession)
        );
    }

    #[test]
    fn rejects_missing_session_bus_before_status_notifier_probe() {
        let mut environment = desktop_environment();
        environment.dbus_session_bus = None;

        assert_eq!(
            diagnose_host(&environment, StatusNotifierProbe::Available),
            Err(HostDiagnostic::NoSessionBus)
        );
    }

    #[test]
    fn reports_gnome_specific_status_notifier_guidance() {
        let environment = desktop_environment();

        let diagnostic = diagnose_host(&environment, StatusNotifierProbe::WatcherMissing)
            .expect_err("GNOME without watcher should be unsupported");

        assert_eq!(
            diagnostic,
            HostDiagnostic::MissingStatusNotifier {
                desktop: DesktopKind::Gnome
            }
        );
        assert!(diagnostic.message().contains("GNOME"));
        assert!(diagnostic.message().contains("StatusNotifier/AppIndicator"));
    }

    #[test]
    fn reports_generic_status_notifier_guidance_for_other_desktops() {
        let environment = HostEnvironment {
            current_desktop: Some("KDE".to_string()),
            ..desktop_environment()
        };

        let diagnostic = diagnose_host(&environment, StatusNotifierProbe::WatcherMissing)
            .expect_err("desktop without watcher should be unsupported");

        assert_eq!(
            diagnostic,
            HostDiagnostic::MissingStatusNotifier {
                desktop: DesktopKind::Other
            }
        );
        assert!(
            diagnostic
                .message()
                .contains("No StatusNotifier/AppIndicator watcher")
        );
    }

    #[test]
    fn reports_invisible_host_separately_from_missing_watcher() {
        let environment = desktop_environment();

        assert_eq!(
            diagnose_host(&environment, StatusNotifierProbe::HostWillNotShow),
            Err(HostDiagnostic::InvisibleStatusNotifierHost {
                desktop: DesktopKind::Gnome
            })
        );
    }

    #[test]
    fn accepts_supported_desktop_with_status_notifier_host() {
        let environment = desktop_environment();

        assert_eq!(
            diagnose_host(&environment, StatusNotifierProbe::Available),
            Ok(())
        );
    }
}
