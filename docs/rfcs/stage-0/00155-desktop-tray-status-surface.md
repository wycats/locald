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
- RFC 0147 mentions GNOME/KDE StatusNotifier integration as a future possibility, but does not specify the tray contract.
- RFC 0104 covers macOS platform support broadly, but does not define a dedicated tray/menu-bar product surface.

A dedicated Tray RFC avoids treating GNOME support as an implementation detail of the macOS path. It establishes the common contract first, then allows each platform to implement it idiomatically.

## 3. Detailed Design

### Terminology

- **Tray agent**: a small desktop process started/stopped by `locald tray ...` that displays locald status in the OS status area.
- **macOS menu bar agent**: the AppKit-backed implementation that lives in the macOS menu bar.
- **Linux StatusNotifier agent**: the DBus-backed implementation for desktops that support StatusNotifierItem/AppIndicator, including KDE and GNOME environments with the appropriate shell support.
- **Tray host**: the desktop environment component that renders tray/status notifier items.

### Semantic parity contract

The tray/menu-bar surface is one user-facing feature with platform-specific renderers, not separate products.

The GNOME/Linux backend should match the macOS backend semantically wherever platform conventions allow:

- The same `locald tray start|stop|status|restart` commands manage the agent.
- The same daemon/service status meanings are presented to the user.
- The same health warnings and setup affordances are exposed when prerequisites are missing.
- The same primary actions exist: open dashboard, restart all services, run setup when needed, and quit/stop the agent.
- Unsupported or degraded platform behavior is reported explicitly instead of silently changing the meaning of the tray feature.

Platform-specific UI details can differ. For example, the macOS surface is a menu bar item and the Linux surface is a StatusNotifier/AppIndicator item. But if a user says "the Linux tray is equivalent to the macOS tray," that should be true for lifecycle, status, health, and action semantics.

### User Experience (UX)

The tray surface should present a small, consistent menu:

- Status summary: daemon not running, running with no services, or N/M services running.
- Health warning when setup/trust/privileged port prerequisites are not satisfied.
- Open Dashboard.
- Restart All Services.
- Run Setup when health checks indicate setup is needed.
- Quit/Stop tray agent.

CLI surface remains:

```text
locald tray start
locald tray stop
locald tray status
locald tray restart
```

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

### Linux/GNOME backend

The Linux backend should use the modern tray/status-notifier path rather than legacy XEmbed-only tray APIs.

Preferred implementation direction:

- Implement a StatusNotifierItem/AppIndicator-compatible agent over DBus, either through a Rust crate or a small local adapter.
- Detect whether a tray host/status notifier watcher is present.
- On GNOME, provide an actionable diagnostic if the shell lacks StatusNotifier/AppIndicator support.
- Keep KDE support in scope when it naturally falls out of the StatusNotifier implementation, but validate against GNOME first because this phase is explicitly GNOME-oriented.

The agent should avoid requiring root. If privileged setup is missing, the tray should surface `Run Setup...` and delegate to the existing `locald admin setup` flow rather than doing privileged work itself.

The Linux backend should not introduce Linux-only meanings for shared tray actions unless the RFC is updated to define those semantics for all platforms. Linux-specific diagnostics are allowed, but they should describe host/platform limitations rather than changing what `locald tray` means.

### Implementation Details

Candidate changes:

- Move the current `#[cfg(target_os = "macos")] mod macos` implementation into a backend module.
- Add a Linux backend module for StatusNotifier/AppIndicator.
- Keep `locald-agent` as the binary name and `locald tray ...` as the user-facing control surface.
- Update non-macOS `locald tray ...` behavior from "macOS-only" to Linux-aware behavior.
- Add focused tests around platform-independent status model formatting and unsupported-environment diagnostics.

## 4. Implementation Plan (Stage 2)

- [ ] Extract a shared tray status model from the current macOS agent.
- [ ] Define shared tray action/status semantics and verify the Linux backend preserves macOS-equivalent user expectations.
- [ ] Refactor the macOS backend behind a backend boundary without changing behavior.
- [ ] Research and select the Rust/Linux StatusNotifier/AppIndicator integration path.
- [ ] Implement Linux agent startup and tray-host detection.
- [ ] Implement GNOME-visible status menu with status, health warning, dashboard, restart, setup, and quit actions.
- [ ] Update `locald tray status` to report Linux agent state.
- [ ] Add tests for shared status model and Linux unsupported-environment diagnostics.
- [ ] Document GNOME requirements and behavior.

## 5. Context Updates (Stage 3)

- [ ] Update `docs/manual/features/cli.md` or the canonical CLI reference to describe `locald tray ...` as a supported desktop integration surface when the platform backend exists.
- [ ] Add or update a manual feature page for desktop tray/menu-bar status.
- [ ] Update architecture docs for the agent/backend split.
- [ ] Update agent context plan/roadmap entries so GNOME tray precedes CNB library extraction.

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

- Which Rust crate or DBus implementation should be used for StatusNotifier/AppIndicator?
- Should pure GNOME without AppIndicator support be treated as unsupported, or should locald eventually ship a GNOME Shell extension?
- How should `locald tray status` distinguish "agent running but no tray host" from "agent not running"?
- Should the Linux tray agent be autostarted by `locald admin setup`, or remain explicitly started by `locald tray start`?

## 9. Future Possibilities

- KDE validation as a first-class acceptance target.
- Autostart integration through XDG desktop autostart files or systemd user units.
- Tray menu entries for per-project attach/detach once RFC 0147 attachments land.
- Editor integration and tray integration sharing a common desktop status model.
