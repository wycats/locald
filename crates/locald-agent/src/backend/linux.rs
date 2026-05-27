#![cfg_attr(not(test), allow(dead_code))]

use std::fmt;

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
