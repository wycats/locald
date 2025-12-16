# Architectural Decisions

## 001. Project Name: locald

**Context**: We need a name for the local development proxy/manager.
**Decision**: Use `locald`.
**Status**: Accepted.

## 002. Language: Rust

**Context**: High performance, safety, and single-binary distribution are desired.
**Decision**: Use Rust.
**Status**: Accepted.

## 003. Configuration: In-Repo

**Context**: Configuration should live with the code.
**Decision**: Use `locald.toml` in the project root.
**Status**: Accepted.

## 004. Architecture: Daemon + CLI

**Context**: Processes need to run in the background, independent of the terminal session.
**Decision**: Split into `locald-server` (daemon) and `locald-cli` (client).
**Status**: Accepted.

## 005. IPC: Unix Domain Sockets

**Context**: The CLI needs a low-latency, reliable way to send commands to the local daemon.
**Decision**: Use Unix Domain Sockets (specifically `/tmp/locald.sock`) with a newline-delimited JSON protocol.
**Status**: Accepted.

## 006. Port Assignment: Dynamic & Env Var

**Context**: Services need to know which port to listen on. Hardcoding ports leads to conflicts.
**Decision**: The daemon dynamically assigns a free port to each service and injects it as the `PORT` environment variable. Services must respect this variable.
**Status**: Accepted.

## 007. Daemonization: Self-Managed (Supersedes 007)

**Context**: Users expect `locald-server` to run in the background. Relying on the CLI to spawn it creates complexity around process groups and signal handling.
**Decision**: The `locald-server` binary handles its own daemonization using the `daemonize` crate. It forks into the background, detaches from the terminal, and manages its own PID file. A `--foreground` flag is provided for debugging.
**Status**: Accepted.

## 008. Daemon Detachment: setsid

**Context**: Simply spawning a background process isn't enough; if the CLI is killed (Ctrl-C), the child might die if it's in the same process group.
**Decision**: Use `setsid` when spawning `locald-server` to create a new session and fully detach it from the CLI's terminal.
**Status**: Accepted.

## 009. Server Idempotency: Socket Check (Supersedes 009)

**Context**: Running `locald-server` multiple times shouldn't cause errors or zombie processes.
**Decision**: The server binary checks if the IPC socket is already active before starting. If it detects a running instance, it exits successfully with a message, ensuring idempotency at the binary level.
**Status**: Accepted.

## 010. Privileged Ports: Capabilities over Root

**Context**: We want to bind port 80 for clean URLs, but running the entire daemon as root violates Axiom 04 (Process Ownership).
**Decision**: The daemon runs as the user. We use `setcap cap_net_bind_service=+ep` on the binary to allow binding low ports. A `locald admin setup` command handles this.
**Status**: Accepted.

## 011. Hosts File: Section Management

**Context**: We need to map local domains to 127.0.0.1. Modifying `/etc/hosts` is risky and requires root.
**Decision**: We implement a safe "Section Manager" that only touches lines between `# BEGIN locald` and `# END locald`. The user runs `locald admin sync-hosts` with sudo to apply changes.
**Status**: Accepted.

## 012. State Persistence: JSON in XDG Data Dir

**Context**: The daemon needs to remember running services across restarts.
**Decision**: Store state in a human-readable JSON file (`state.json`) located in the standard XDG data directory (`~/.local/share/locald/`).
**Status**: Accepted.

## 013. Process Recovery: Kill & Restart

**Context**: When the daemon restarts, it finds "zombie" processes from the previous session. Adopting them is complex due to lost I/O pipes.
**Decision**: The daemon kills the old PID (if running) and restarts the service from scratch. This ensures a clean state and re-establishes log capture.
**Status**: Accepted.

## 014. Documentation: Persona-Based Structure

**Context**: Documentation was becoming a mix of tutorials and references without a clear audience.
**Decision**: Structure documentation explicitly around three personas: App Builder (Guides), Power User (Reference), and Contributor (Internals).
**Status**: Accepted.

## 015. CLI: Interactive Init

**Context**: New users struggle to create valid `locald.toml` files manually.
**Decision**: Implement `locald init` using `dialoguer` to guide users through project creation.
**Status**: Accepted.

## 016. CLI: TUI Monitor

**Context**: Users want a real-time dashboard of running services without leaving the terminal.
**Decision**: Implement `locald monitor` using `ratatui` (community fork of tui-rs) for a robust TUI experience.
**Status**: Accepted.

## 017. Dependency Resolution: Topological Sort

**Context**: Services may depend on other services (e.g., API depends on DB). We need to start them in the correct order.
**Decision**: Use a topological sort (Kahn's algorithm) to determine the startup sequence. For the MVP, we only guarantee the _spawn_ order, not the "ready" state (which would require health checks).
**Status**: Accepted.

## 018. Documentation: Sticky Language Tabs

**Context**: The "Common Patterns" guide needs to show examples for multiple languages (Node, Python, Go, Rust). Users typically care about one language at a time.
**Decision**: Use Astro Starlight's `<Tabs syncKey="lang">` component. This persists the user's language selection across different examples on the page (and potentially across pages), reducing cognitive load.
**Status**: Accepted.

## 019. Health Checks: Zero-Config Hierarchy

**Context**: Users shouldn't have to manually configure health checks for standard setups.
**Decision**: Implement a hierarchy of detection strategies:

1. Docker Native (`HEALTHCHECK` in image)
2. `sd_notify` (if app supports systemd notification)
3. TCP Probe (if port is defined)
4. Explicit `health_check` command (override)
   **Status**: Accepted.

## 020. Notify Protocol: Unix Datagram

**Context**: We need a standard way for apps to signal readiness. `systemd`'s `sd_notify` is the de-facto standard.
**Decision**: Implement a Unix Datagram socket server that mimics `systemd`'s notification socket. Inject `NOTIFY_SOCKET` env var into child processes.
**Status**: Accepted.

## 021. Docker Health: Polling

**Context**: We need to know when a Docker container is healthy.
**Decision**: Poll `inspect_container` every few seconds to check the health status. This is simpler than listening to the Docker event stream for this phase.
**Status**: Accepted.

## 023. SSL Strategy: Pure Rust Stack

**Context**: We need to support `.dev` domains which require HTTPS (HSTS). We want to avoid external dependencies like `mkcert` or `openssl` binaries to maintain our "Single Binary" philosophy.
**Decision**: Use `rcgen` for certificate generation and `devcert` (or similar logic) for trust store injection. Implement `ResolvesServerCert` in `rustls` to sign certificates on-the-fly during the TLS handshake.
**Status**: Accepted.

## 024. Default Domain: .localhost

**Context**: `.local` domains rely on mDNS which is flaky on macOS and not treated as a Secure Context by browsers.
**Decision**: Switch the default domain suffix from `.local` to `.localhost`. This provides reliability and Secure Context benefits without requiring SSL configuration.
**Status**: Accepted.

## 025. Single Binary Distribution (Supersedes 004)

**Context**: Distributing and updating two separate binaries (`locald-server` and `locald-cli`) is cumbersome for users and complicates version synchronization.
**Decision**: Merge the server logic into the CLI binary. The `locald` binary now contains both the client and the server (invoked via `locald server start`). This simplifies distribution to a single executable.
**Status**: Accepted.

## 026. Global Configuration

**Context**: Users need to configure system-wide behavior, such as port binding preferences (privileged vs. unprivileged), that applies across all projects.
**Decision**: Introduce a global configuration file (e.g., `~/.config/locald/config.toml`) managed by `GlobalConfig`. This allows users to opt-out of privileged port binding or enable fallback behavior without modifying per-project configs.
**Status**: Accepted.

## 027. Managed Postgres: postgresql_embedded

**Context**: Users need a zero-config database for local development without requiring Docker or manual installation.
**Decision**: Use the `postgresql_embedded` crate. It handles downloading the correct binary for the OS/Arch, initializing the data directory, and starting the server on a dynamic port.
**Status**: Accepted.

## 028. Service Configuration: Typed Enum

**Context**: Different service types (Exec, Postgres) have different configuration requirements (e.g., Postgres doesn't need a `command`).
**Decision**: Refactor `ServiceConfig` to use a Serde tagged enum (`type` field). This allows strict validation and type-safe configuration for different service runners.
**Status**: Accepted.

## 029. Service Reset: Explicit Command

**Context**: Stateful services like Postgres sometimes need to be wiped clean. Users shouldn't have to manually find and delete hidden directories.
**Decision**: Implement `locald service reset <name>`. This command stops the service, deletes its data directory (if applicable), and restarts it.
**Status**: Accepted.

## 030. Gitignore: Automated Management

**Context**: `locald` creates a `.locald` directory for state and logs. Users often forget to add this to `.gitignore`, leading to accidental commits of local state.
**Decision**: `locald init` and `locald service add` will automatically check for and append `.locald/` to the project's `.gitignore` file if it exists.
**Status**: Accepted.

## 031. Dashboard Stack: Svelte 5 & xterm.js

**Context**: The dashboard needs to be responsive, type-safe, and capable of rendering high-frequency log streams without bogging down the DOM.
**Decision**: Use Svelte 5 for its fine-grained reactivity and performance. Use `xterm.js` for canvas-based terminal rendering, which is significantly more performant than DOM-based log rendering and supports full ANSI codes.
**Status**: Accepted.

## 032. The Manifesto Structure

**Context**: The design axioms were a flat list that was growing unwieldy.
**Decision**: Group axioms into three pillars: **Experience** (User-facing), **Architecture** (Internal structure), and **Environment** (System integration).
**Consequences**: Makes the philosophy easier to digest and ensures we cover all bases when designing new features.

## 033. Ephemeral Runtime, Persistent Context (Axiom 7)

**Context**: Users need to debug crashes, but processes are transient.
**Decision**: Explicitly decouple the _runtime state_ (PID, port) from the _contextual state_ (logs, history, config). The system must preserve context even when the runtime is gone.
**Consequences**: Requires persistent storage for logs and history, not just in-memory state.

## 034. Dashboard as Workspace

**Context**: The dashboard was treated as a passive monitor.
**Decision**: The dashboard is the primary _workspace_ for development. It must support active control (restart, stop) and organization (grouping, filtering).
**Consequences**: Drives the requirements for Phase 24 (Ergonomics) and Phase 25 (Constellations).

## 035. Advanced Proxying Strategy

**Context**: Simple port mapping is insufficient for modern apps (microservices, H2/H3).
**Decision**: `locald` will support path-based routing (`/api` -> Service A) and modern protocols (HTTP/2, HTTP/3) to provide a production-grade networking layer.
**Consequences**: Will require a significant upgrade to the proxy implementation in a future phase.

## 036. Project Registry: Centralized Tracking

**Context**: The user needs to manage multiple projects and know which ones are active or "pinned" for auto-start.
**Decision**: Implement a centralized `registry.json` in the user's data directory. This file tracks known projects, their paths, and their pinned status. It serves as the source of truth for multi-project management commands.
**Status**: Accepted.

## 037. Sandbox Environments: Explicit Isolation

**Context**: Developers and AI agents need to test `locald` itself without corrupting the main user environment or leaking state.
**Decision**: Implement a `--sandbox <NAME>` flag that isolates `XDG_*` directories and the IPC socket. Enforce strict safety by panicking if `LOCALD_SOCKET` is set without the sandbox flag.
**Status**: Accepted.
