<!-- exo:155 ulid:01kskjh57267r64qwr6jhjg63s -->

# RFC 155: Desktop Tray Status Surface

## 1. Summary

Define `locald`'s desktop tray/menu-bar status surface as a cross-platform agent pattern.

The current implementation is a macOS-only `locald-agent` menu bar app controlled by `locald tray ...`. This RFC generalizes that surface so the same user-facing concept can cover:

- macOS menu bar support through the existing AppKit/tray agent.
- Linux desktop tray support through a GNOME/KDE-compatible StatusNotifier/AppIndicator agent.

The immediate phase target is GNOME/Linux support, while preserving the existing macOS tray contract. The Linux implementation should be semantically equivalent to macOS wherever users would reasonably understand the two platform surfaces as the same `locald tray` feature.

## 2. Motivation

After UX polish, the next user-visible gap is desktop presence: users should be able to see whether `locald` is running, whether services are healthy, and quickly open the dashboard or run setup without living in a terminal.

There is already partial tray infrastructure:

- `locald-agent` exists and is currently macOS-only.
- `locald tray start|stop|status|restart` exists as a CLI integration surface.
- RFC 0147 mentions GNOME/KDE StatusNotifier integration as future desktop context, but does not specify the tray contract.
- RFC 0104 covers macOS platform support broadly, but does not define a dedicated tray/menu-bar product surface.

A dedicated Tray RFC avoids treating GNOME support as an implementation detail of the macOS path. It establishes the common contract first, then allows each platform to implement it idiomatically.

## 3. Detailed Design

### Terminology

- **Tray agent**: a small desktop process started/stopped by `locald tray ...` that displays locald status in the OS status area.
- **macOS menu bar agent**: the AppKit-backed implementation that lives in the macOS menu bar.
- **Linux StatusNotifier agent**: the DBus-backed implementation for desktops that support StatusNotifierItem/AppIndicator, including KDE and GNOME environments with the appropriate shell support.
- **Tray host**: the desktop environment component that renders tray/status notifier items.

### Stage 1 contract boundary

Stage 1 decides the user-facing contract for the desktop tray surface. It records the Linux host and autostart contract without making Distrobox, toolbox, bkt, or other development-container environments part of locald's product logic.

The contract is:

- `locald tray ...` is the lifecycle surface for desktop tray/menu-bar agents.
- The tray agent exposes a shared locald status model across supported platforms.
- Unsupported or degraded host environments are explicit states with actionable diagnostics.
- Linux support in this RFC means desktops with StatusNotifierItem/AppIndicator-compatible tray host support.
- Pure GNOME without StatusNotifier/AppIndicator support is unsupported by this RFC unless the user installs/enables host support.
- Linux autostart uses user-scoped XDG autostart files via `locald tray autostart ...`; systemd user units remain deferred future work.
- Linux autostart pins a host-visible `locald` launcher path. The default is the current executable, and `locald tray autostart enable --locald-path <path>` lets users pin an explicit host path when their shell command is a container or environment wrapper.

### Platform support matrix

| Platform/desktop host                                           | Stage 1 contract status  | Expected behavior                                                                                                              |
| --------------------------------------------------------------- | ------------------------ | ------------------------------------------------------------------------------------------------------------------------------ |
| macOS with the existing menu bar backend                        | Supported                | `locald tray ...` manages the existing menu bar agent using the shared status/action semantics.                                |
| Linux desktop with StatusNotifierItem/AppIndicator host support | Supported target         | `locald tray start` starts a visible tray agent after host capability checks pass.                                             |
| GNOME with distro/extension AppIndicator support enabled        | Supported target         | The Linux agent uses the available StatusNotifier/AppIndicator host and presents the shared tray menu.                         |
| Pure GNOME without StatusNotifier/AppIndicator support          | Unsupported for this RFC | `locald tray start` fails with an actionable diagnostic. It must not report success while starting a headless invisible agent. |
| Linux without a graphical session or without a tray host        | Unsupported for this RFC | `locald tray start` fails with an actionable diagnostic naming the missing desktop/tray-host requirement.                      |
| Other operating systems                                         | Unsupported              | `locald tray ...` reports that no tray backend exists for the platform.                                                        |

This matrix is a contract requirement. Stage 2 may add implementation detail, but it must not silently weaken unsupported/degraded cases into successful no-op or headless behavior.

### Semantic parity contract

The tray/menu-bar surface is one user-facing feature with platform-specific renderers, not separate products.

The GNOME/Linux backend should match the macOS backend semantically wherever platform conventions allow:

- The same `locald tray start|stop|status|restart` commands manage the agent.
- The same daemon/service status meanings are presented to the user.
- The same health warnings and setup affordances are exposed when prerequisites are missing.
- The same primary actions exist: open dashboard, restart all services, run setup when needed, and quit/stop the agent.
- Unsupported or degraded platform behavior is reported explicitly instead of silently changing the meaning of the tray feature.

Platform-specific UI details can differ. For example, the macOS surface is a menu bar item and the Linux surface is a StatusNotifier/AppIndicator item. But if a user says "the Linux tray is equivalent to the macOS tray," that should be true for lifecycle, status, health, and action semantics.

### Shared status model

The tray status model must distinguish at least these states before platform rendering:

1. **Unsupported platform/host**: no supported platform backend or no visible tray host is available.
2. **Agent not installed**: the tray agent binary or launcher integration required by the platform is missing.
3. **Agent installed but not running**: locald can find the tray agent but no live agent process/session is active.
4. **Agent running with visible host**: the tray agent is running and has registered with a visible tray/menu-bar host.
5. **Agent running without visible host**: the process is running, but the status item is not visible because host registration failed, the watcher disappeared, or the desktop does not expose a tray host.

`locald tray status` must preserve these distinctions. It may use platform-specific wording, but the CLI output must make the operational state and next action clear.

Daemon/service status shown inside the tray menu is separate from tray-agent lifecycle status. The menu summary should continue to distinguish daemon not running, daemon running with no services, and N/M services running.

### CLI lifecycle and diagnostics

The CLI contract is:

```text
locald tray start
locald tray stop
locald tray status
locald tray restart
```

- `start` starts the platform tray agent only when the platform and visible host requirements are satisfied or can be verified during startup.
- `stop` stops the tray agent without stopping the locald daemon or project services.
- `restart` is equivalent to `stop` followed by `start`, preserving the same unsupported-host checks as `start`.
- `status` reports both tray-agent lifecycle state and whether the current platform/host can show a visible status item.

Unsupported or degraded cases must return a non-success result for lifecycle commands that cannot deliver a visible tray/menu-bar surface. For example, pure GNOME without AppIndicator support should fail with a message that names the missing StatusNotifier/AppIndicator host support and suggests enabling/installing it or using `locald` from the CLI/dashboard instead.

The contract does not allow silent fallback to an invisible background process. If `locald tray start` succeeds, a supported visible desktop status surface must be present or in the process of registering.

### Menu action semantics

The shared tray menu actions have these meanings:

- **Open Dashboard** opens the local dashboard URL through the platform's normal browser-opening mechanism. It does not start services by itself.
- **Restart All Services** delegates to the existing locald restart semantics for the active locald environment. It should surface failure rather than silently ignoring restart errors.
- **Run Setup** appears when health checks indicate setup/trust/privileged-port prerequisites are missing. It delegates to the existing `locald admin setup` flow rather than performing privileged work inside the tray agent.
- **Quit/Stop Tray Agent** stops only the tray agent. It does not stop the daemon or project services.

Linux-specific diagnostics may explain host limitations, but Linux must not introduce different meanings for shared actions.

### macOS parity checklist

Before Stage 3, Linux support should be checked against current macOS behavior for:

- `locald tray start|stop|status|restart` lifecycle semantics.
- Visibility and wording of daemon/service status summaries.
- Health/setup affordances when privileged setup or certificate trust is missing.
- Dashboard, restart, setup, and quit action behavior.
- Explicit unsupported/degraded diagnostics rather than silent success.
- Preservation of the current macOS menu bar behavior while introducing the shared model.

### User Experience (UX)

The tray surface should present a small, consistent menu:

- Status summary: daemon not running, running with no services, or N/M services running.
- Health warning when setup/trust/privileged port prerequisites are not satisfied.
- Open Dashboard.
- Restart All Services.
- Run Setup when health checks indicate setup is needed.
- Quit/Stop tray agent.

On unsupported or unprepared Linux desktops, `locald tray start` should fail with a clear, actionable message, for example explaining that GNOME may require StatusNotifier/AppIndicator support to be enabled or installed. It should not silently start a headless background process with no visible status item.

### Architecture

`locald-agent` becomes a platform-dispatching binary instead of a macOS-only binary:

```text
locald tray start
  -> installs/starts locald-agent using the platform launcher
  -> locald-agent selects backend:
       macOS: AppKit/menu bar backend
       Linux: StatusNotifier/AppIndicator backend
       other: explicit unsupported message
```

The agent continues to communicate with the daemon through existing locald IPC rather than introducing a second control plane.

Shared behavior should be factored into platform-independent code where possible:

- Poll daemon status.
- Convert service state into a tray status model.
- Run health checks that do not require root.
- Invoke existing CLI/IPC actions for dashboard open, restart, setup, and quit.

Platform backends are responsible only for rendering the menu/status item and integrating with the OS event loop.

### macOS backend

The macOS backend continues to use the existing `tray-icon`, `muda`, and AppKit event loop implementation.

This RFC does not require redesigning the macOS agent. It records the existing backend as one implementation of the shared tray contract.

Current macOS behavior forms the parity baseline for the Linux/GNOME backend:

- `locald-agent` is compiled as a macOS menu bar process. Non-macOS builds currently print `locald-agent is macOS-only`.
- `locald tray start` requires the user LaunchAgent at `~/Library/LaunchAgents/com.locald.agent.plist`. If it is missing, interactive terminals offer to run `sudo locald admin setup`; otherwise the command fails with `LaunchAgent not installed. Run locald admin setup first.`
- `locald tray start` verifies the embedded agent binary and updates it when stale before invoking `launchctl start com.locald.agent`.
- `locald tray stop` invokes `launchctl stop com.locald.agent` and does not stop the daemon or project services.
- `locald tray restart` requires the LaunchAgent, then performs `launchctl stop` followed by `launchctl start`.
- `locald tray status` uses `launchctl list com.locald.agent` to distinguish loaded/running from loaded/not-running and not-loaded states, and reports the pinned daemon path when configured.
- The menu bar item presents daemon status as `Status: checking...`, `Status: not running`, `Status: running (no services)`, or `Status: N/M running`.
- The agent polls daemon status through existing locald IPC (`Ping`, then `Status`) rather than introducing a separate control plane.
- If the daemon is not running, the agent attempts to start it no more often than every 30 seconds, using the pinned `LOCALD_DAEMON_PATH` value from the LaunchAgent. It logs and skips auto-start when the pinned path is missing or invalid rather than guessing from `PATH`.
- Health checks are non-root probes for privileged helper installation, port 80 reachability, and local CA trust. When unhealthy, the menu shows a warning label and enables `Run Setup...`.
- `Run Setup...` first attempts setup through the privileged XPC helper, then falls back to opening Terminal.app with `locald admin setup`.
- `Open Dashboard` opens `http://locald.localhost` with the platform browser-opening command.
- `Restart All Services` sends the existing locald IPC `RestartAll` request.
- `Quit` terminates only the menu bar agent.

Linux parity does not require copying macOS launchctl, LaunchAgent, AppKit, Terminal.app, or XPC mechanics. It requires preserving the user-visible lifecycle, status, health, and action semantics above with Linux-appropriate host integration and diagnostics.

### Linux/GNOME backend

The Linux backend should use the modern tray/status-notifier path rather than legacy XEmbed-only tray APIs.

Stage 2 should determine the concrete Linux implementation path. The contract requires the implementation to:

- Implement a StatusNotifierItem/AppIndicator-compatible agent over DBus, either through a Rust crate or a small local adapter selected during Stage 2 research.
- Detect whether a tray host/status notifier watcher is present.
- On GNOME, provide an actionable diagnostic if the shell lacks StatusNotifier/AppIndicator support.
- Keep KDE support in scope when it naturally falls out of the StatusNotifier implementation, but validate against GNOME first because this phase is explicitly GNOME-oriented.

Stage 2 selects `ksni` as the first implementation path for the Linux backend.

Rationale:

- `ksni` implements the KDE/freedesktop StatusNotifierItem specification directly over DBus, matching the RFC's StatusNotifier/AppIndicator contract without routing the feature through GTK or a native shared-library AppIndicator binding.
- `ksni` exposes watcher and visibility failures as typed states such as watcher registration errors and `WontShow`, which maps directly to the RFC requirement that unsupported or invisible tray hosts fail with actionable diagnostics instead of silently starting a headless process.
- `ksni` supports Tokio and async-io runtimes. The first locald integration should use Tokio because locald already uses Tokio elsewhere and the tray agent already accepts platform-specific runtime dependencies.
- `ksni` keeps KDE naturally in scope while allowing GNOME-first validation because it targets the shared StatusNotifierItem protocol rather than a GNOME-only shell extension or an AppIndicator-only FFI surface.

Rejected paths for the first Linux backend:

- `tray-icon` remains appropriate for the existing macOS menu-bar implementation, but its Linux path is a higher-level GTK/libappindicator stack with extra system library and event-loop requirements. That makes visible-host diagnostics and DBus watcher handling less precise for the first GNOME implementation slice.
- `appindicator3`, `libayatana-appindicator`, and related bindings expose native AppIndicator/Ayatana libraries through GTK or FFI. They are useful comparison points, but they add system shared-library coupling and are less aligned with locald's preference for precise primitives.
- Raw `zbus` is the fallback if `ksni` blocks required behavior, but hand-implementing StatusNotifierItem, menu export, watcher registration, and lifecycle handling from raw DBus is unnecessary for the first implementation pass.

Implementation guidance for the selected path:

- Add Linux-only dependencies for `ksni` and the minimal Tokio runtime features needed by the backend.
- Do not enable `ksni`'s "assume SNI available" behavior for normal startup; locald should fail fast when no watcher/host can show a visible tray item.
- Map `ksni` watcher registration failures, `WontShow`, and watcher-offline callbacks into locald's unsupported-host diagnostics and `locald tray status` lifecycle states.
- Keep a small local adapter around `ksni` so shared locald status/action semantics remain owned by locald rather than by the crate API.

The agent should avoid requiring root. If privileged setup is missing, the tray should surface `Run Setup...` and delegate to the existing `locald admin setup` flow rather than doing privileged work itself.

The Linux backend should not introduce Linux-only meanings for shared tray actions unless the RFC is updated to define those semantics for all platforms. Linux-specific diagnostics are allowed, but they should describe host/platform limitations rather than changing what `locald tray` means.

### Implementation Details

Candidate Stage 2 changes:

- Move the current `#[cfg(target_os = "macos")] mod macos` implementation into a backend module.
- Add a Linux backend module for StatusNotifier/AppIndicator.
- Keep `locald-agent` as the binary name and `locald tray ...` as the user-facing control surface.
- Update non-macOS `locald tray ...` behavior from "macOS-only" to Linux-aware behavior.
- Add focused tests around platform-independent status model formatting and unsupported-environment diagnostics.

## 4. Implementation Plan (Stage 2)

- [x] Extract a shared tray status model from the current macOS agent, preserving the state distinctions defined in this RFC.
- [x] Define shared tray action/status semantics in code and verify the Linux backend preserves macOS-equivalent user expectations.
- [x] Refactor the macOS backend behind a backend boundary without changing behavior.
- [x] Research and select the Rust/Linux StatusNotifier/AppIndicator integration path.
- [x] Implement Linux agent startup and visible tray-host detection.
- [x] Implement explicit unsupported-host diagnostics for pure GNOME without AppIndicator support, headless Linux sessions, and unsupported operating systems.
- [x] Implement GNOME-visible status menu with status, health warning, dashboard, restart, setup, and quit actions.
- [x] Update `locald tray status` to report platform support, installation, process, and visible-host state.
- [x] Add tests for shared status model and Linux unsupported-environment diagnostics.
- [x] Document GNOME requirements and behavior.

## 5. Context Updates (Stage 3)

- [x] Update `docs/manual/features/cli.md` or the canonical CLI reference to describe `locald tray ...` as a supported desktop integration surface when the platform backend exists.
- [x] Add or update a manual feature page for desktop tray/menu-bar status.
- [x] Update architecture docs for the agent/backend split.
- [x] Update agent context plan/roadmap entries so GNOME tray precedes CNB library extraction.

## 6. Drawbacks

- Desktop tray behavior varies significantly across Linux desktops.
- GNOME may require an extension or distro-provided AppIndicator support, which makes the feature less universal than the macOS menu bar.
- A tray agent adds another long-lived process to maintain and test.
- DBus/status notifier behavior can be harder to validate in headless CI.

## 7. Alternatives

1. **Keep tray macOS-only.** Simpler, but leaves Linux without an equivalent desktop status surface.
2. **Use only the dashboard.** Avoids tray complexity, but requires users to know the daemon/dashboard state before opening it.
3. **Build a GNOME Shell extension.** More native in pure GNOME, but substantially increases maintenance and packaging scope.
4. **Use notifications only.** Good for events, but not a persistent status/control surface.

## 8. Unresolved Questions

- What is the best automated/manual validation strategy for DBus StatusNotifier behavior in CI and developer environments?
- What exact diagnostic wording should be used for each unsupported-host state?
- Should locald eventually ship or recommend a GNOME Shell extension for pure GNOME environments?

## 9. Future Possibilities

- KDE validation as a first-class acceptance target.
- Systemd user-unit autostart support.
- Optional GNOME Shell extension support for pure GNOME environments.
- Tray menu entries for per-project attach/detach once RFC 0147 attachments land.
- Editor integration and tray integration sharing a common desktop status model.
