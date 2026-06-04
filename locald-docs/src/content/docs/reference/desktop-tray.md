---
title: Desktop Tray
description: Desktop tray and menu-bar status agent behavior on macOS and Linux.
---

The desktop tray/menu-bar agent is an optional status surface for `locald`. It keeps the daemon-first workflow visible while you are using the desktop: daemon status, service health, setup warnings, and quick actions are available without keeping a terminal focused.

The CLI lifecycle surface is:

```bash
locald tray start
locald tray stop
locald tray status
locald tray restart
locald tray autostart enable
locald tray autostart enable --locald-path ~/.cargo/bin/locald
locald tray autostart disable
locald tray autostart status
```

## What the tray shows

The tray/menu-bar menu uses the same status semantics across platforms:

- `Status: checking...`
- `Status: not running`
- `Status: running (no services)`
- `Status: N/M running`

It also surfaces local host readiness checks that matter to the desktop workflow, including privileged helper setup, port 80 reachability, and local CA trust. When setup is needed, the tray provides a setup action that delegates to `locald admin setup` rather than doing privileged work inside the agent.

Common actions:

- **Open Dashboard** opens `http://locald.localhost` in the platform browser.
- **Restart All Services** sends locald's existing restart-all request to the daemon.
- **Run Setup** launches the existing privileged setup flow when host readiness is incomplete.
- **Quit** stops only the tray/menu-bar agent. It does not stop the locald daemon or project services.

## macOS

On macOS, `locald-agent` runs as a menu-bar app. `sudo locald admin setup` installs the user LaunchAgent that starts the agent at login and pins the `locald` executable path used by the agent.

Use:

```bash
locald tray start
locald tray status
locald tray stop
locald tray restart
```

If the LaunchAgent is missing, `locald tray start` asks interactive terminals whether to run setup. Non-interactive shells fail with a clear message to run `locald admin setup` first.

## Linux desktop sessions

On Linux, `locald-agent` uses the StatusNotifier/AppIndicator desktop tray protocol. This covers desktops with a visible StatusNotifier host, including KDE and GNOME sessions where AppIndicator/StatusNotifier support is installed or enabled.

Requirements for `locald tray start` on Linux:

- run as the desktop user, not through `sudo`,
- a graphical session with `WAYLAND_DISPLAY` or `DISPLAY`,
- a D-Bus session bus with `DBUS_SESSION_BUS_ADDRESS`,
- a visible StatusNotifier/AppIndicator tray host,
- a provisioned `locald-agent` in locald's data directory (installed automatically from the embedded agent bytes), or a dev override via `LOCALD_AGENT_PATH`.

Pure GNOME sessions without AppIndicator/StatusNotifier support are unsupported for this tray backend. Install or enable GNOME AppIndicator/StatusNotifier support, then run `locald tray start` again.

Use `locald tray autostart enable` to write a user-scoped XDG autostart entry at `$XDG_CONFIG_HOME/autostart/com.locald.agent.desktop`, or `~/.config/autostart/com.locald.agent.desktop` when `XDG_CONFIG_HOME` is unset. It runs the tray agent at login without requiring systemd user units or root access.

The autostart entry pins the `locald` launcher path that the tray agent uses for setup and daemon actions. By default, it pins the current executable. If your shell resolves `locald` through a dev-container or Distrobox wrapper, pass an explicit host-visible launcher instead:

```bash
locald tray autostart enable --locald-path ~/.cargo/bin/locald
```

Disabling autostart removes the XDG entry but does not stop a currently running tray agent. Use `locald tray stop` for that.

## Diagnostics

`locald tray start` must not silently start a headless invisible tray process. If Linux cannot show a visible tray item, startup fails with an actionable diagnostic.

`locald tray status` reports:

- whether the tray agent process is running,
- whether a stale PID file was cleaned up,
- where the agent binary was found,
- which `locald` daemon path is pinned,
- whether Linux autostart is enabled and which daemon path it will use,
- whether the current shell has the graphical session and D-Bus session bus needed for tray startup.

If the Linux agent exits during startup, the CLI prints the recent contents of `/tmp/locald-agent.log` and points at that log for details.
