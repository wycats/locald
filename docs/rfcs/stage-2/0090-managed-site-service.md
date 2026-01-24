# Managed Site Service & The Locald Watcher

## Context

We are evolving the `locald serve` concept into a robust **Managed Service Architecture** for static content. This addresses the need for a "Just Works" documentation/site workflow that handles building, serving, and watching without the fragility of ad-hoc scripts or the performance pitfalls of traditional user-space watchers.

## Core Concepts

### 1. The "Site" Service Type

We introduce `type = "site"` as a first-class citizen in `locald.toml`. This service type orchestrates three distinct components:

1.  **The Server**: A high-availability static file server (our embedded `axum` server) that binds once and stays up.
2.  **The Builder**: A task (e.g., `cargo doc`) that runs to generate content.
3.  **The Watcher**: A sophisticated monitor that triggers the Builder on relevant file changes.

### 2. The Locald Watcher (Kernel-Level Monitoring)

To solve the "bogged down laptop" problem caused by watching massive directories (`node_modules`, `target`), we will leverage OS-specific kernel features via our privileged shim.

- **Linux**: **Fanotify** (or eBPF). Allows us to monitor filesystem events globally or on mount points with high efficiency, ignoring irrelevant paths without traversing them.
- **macOS**: **FSEvents**. The native API for efficient directory monitoring.
- **Windows**: **ReadDirectoryChangesW** (IOCP).

**Design Goal**: The watcher should be "Zero-Config Hygiene". It should inherently understand build artifacts and ignore them, requiring minimal to no manual exclusion lists from the user.

### 3. Build Status UX & "The Toolbar"

When a build fails, the server should not crash or serve a 503. Instead, it should continue serving the **stale** (last known good) content.

To communicate state, we will inject a **Status Toolbar** into the HTML response:

1.  **Floating Action Button (FAB)**: A small, draggable circular indicator that shows the current status (e.g., Green=Good, Yellow=Building, Red=Failed).
2.  **Expandable Toolbar**: Clicking the FAB animates out a toolbar containing:
    - Build status details.
    - Error logs (if failed).
    - Build duration.
3.  **Non-Intrusive**: The user can hide/collapse it or move it around to avoid blocking content.
4.  **Implementation**: We intercept HTML responses to inject a script/style block that connects to a `locald` WebSocket for real-time updates.

## Implementation Plan

### Phase 1: The "Site" Service (Standard Watcher)

To ship value quickly, we will implement the `site` service logic using the standard `notify` crate initially.

- **Constraint**: We will implement sensible default ignores (git-ignore style) to mitigate performance issues on Linux, but we acknowledge this is a stopgap until Phase 2.
- **Concurrency**: Implement the "Debounce & Restart" strategy. If a file changes during a build, cancel the current build and queue a new one.

### Phase 2: The Kernel Watcher

We will develop the privileged watcher infrastructure in parallel.

- Extend `locald-shim` to expose a watching capability.
- On Linux, implement the Fanotify backend.
- Update `locald-server` to prefer the shim-based watcher when available.

## Configuration Strawman

```toml
[[services]]
name = "rustdocs"
type = "site"
path = "target/doc"
build = "cargo doc --workspace --no-deps"
# Optional: defaults to smart root detection
watch = ["src", "Cargo.toml"]
```

## Open Questions / Refinements

1.  **Overlay Injection**: How do we robustly inject the overlay? We likely need a small JavaScript client that connects to a WebSocket on the `locald` server to listen for build status events.
2.  **Watcher Generalization**: While we start with `type = "site"`, the watcher logic should be designed as a reusable primitive for future `type = "exec"` services that need restart-on-change behavior.
