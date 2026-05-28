# locald-agent

Desktop tray/menu-bar agent for locald. Shows daemon status, host health, and quick actions.

Platform backends:

- macOS: AppKit/menu-bar backend managed through the user LaunchAgent installed by `locald admin setup`.
- Linux: StatusNotifier/AppIndicator backend for desktop sessions with a graphical display, a D-Bus session bus, and visible tray host support such as GNOME with AppIndicator/StatusNotifier enabled.

The agent is controlled through `locald tray start|stop|status|restart`. Unsupported Linux sessions fail explicitly instead of starting an invisible background process.
