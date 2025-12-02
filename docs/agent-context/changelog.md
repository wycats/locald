<!-- agent-template start -->

# Changelog

History of completed phases and key changes.

<!-- agent-template end -->

## Phase 14: Dogfooding & Polish (2025-12-02)

**Goal**: Smooth out the rough edges before broader adoption. Focus on error messages, CLI output, and common workflow friction.

**Work Completed**:

- **Unified Binary**: Renamed `locald-cli` to `locald` for better ergonomics.
- **CLI Polish**: Added colored output, improved tables, and better feedback for `start`/`stop`.
- **New Feature**: Implemented `locald run <command>` for quick, config-free service execution.
- **Code Quality**: Established strict linting (`clippy`), pre-commit hooks (`lefthook`), and CI/CD (GitHub Actions).
- **Verification**: Verified multi-project workflow (running multiple projects simultaneously).
- **SSL Design**: Researched and designed a "Pure Rust" SSL stack for `.dev` domain support.

## Phase 13: Smart Health Checks (2025-12-01)

**Goal**: Ensure services are ready before dependents start, using "Zero-Config" detection where possible.

**Work Completed**:

- **Health Check Hierarchy**: Implemented a prioritized strategy: Docker Health -> `sd_notify` -> TCP Probe -> Explicit Config.
- **Notify Socket**: Implemented `NotifyServer` to handle `sd_notify` messages (`READY=1`) from services.
- **Docker Integration**: Added support for Docker native `HEALTHCHECK` polling via `bollard`.
- **TCP Probe**: Implemented a fallback TCP connection check for services with ports.
- **Dependency Gating**: Updated `ProcessManager` to wait for a service to be `Healthy` before starting its dependents.
- **CLI**: Updated `locald status` to display `HEALTH` status and `SOURCE`.

## Phase 11: Documentation & Persona Alignment (2025-12-01)

**Goal**: Ensure documentation fully serves the defined personas before adding more complexity.

**Work Completed**:

- **Fresh Eyes Review**: Audited all documentation against the "App Builder", "Power User", and "Contributor" personas.
- **App Builder**:
  - Updated "Getting Started" to use `locald init`.
  - Created "Common Patterns" guide with copy-pasteable examples for Node, Python, Go, and Rust.
  - Implemented sticky language tabs using Starlight's `<Tabs syncKey="lang">`.
- **Power User**:
  - Updated "Configuration Reference" to include `depends_on`.
  - Documented environment variables injected by `locald`.
- **Contributor**:
  - Updated "Architecture Overview" to reflect dependency resolution and state persistence.
- **Verification**: Built and verified the documentation site.

## Phase 4: Local DNS & Routing (2025-11-30)

- **Reverse Proxy**: Implemented a `hyper`-based reverse proxy in `locald-server` listening on port 80 (with 8080 fallback).
- **Domain Routing**: Requests are routed to services based on the `Host` header matching the project's `domain` config.
- **Hosts File Management**: Implemented `HostsFileSection` manager in `locald-core` to safely manage local domains in `/etc/hosts`.
- **Admin Commands**: Added `locald admin setup` (for `setcap`) and `locald admin sync-hosts` to `locald-cli`.
- **Documentation**: Added "DNS and Domains" guide and updated CLI reference.

## Phase 3: Documentation & Design Refinement (2025-11-30)

- Established documentation site using Astro Starlight in `locald-docs/`.
- Documented core concepts: Interaction Modes, Personas, and Architecture.
- Created "Getting Started" guide and CLI/Configuration references.
- Refined design axioms and interaction modes based on "Fresh Eyes" review.
- **Self-Hosting**: Configured `locald` to host its own documentation (`locald-docs`).
- **Robustness**: Implemented `setsid` for true daemon detachment and server idempotency.
- **CLI Improvements**: Added `shutdown` command, context-aware `stop`, and URL display in `status`.

## Phase 2: Process Management (2025-11-30)

- Implemented `ProcessManager` in `locald-server` to spawn and manage child processes.
- Added `start`, `stop`, and `status` commands to `locald-cli` and IPC protocol.
- Implemented dynamic port assignment and `PORT` environment variable injection.
- Implemented `locald server` command to spawn the daemon in the background.
- Verified functionality with a dummy service.

## Phase 1: Scaffolding & Daemon Basics (2025-11-30)

- Initialized Rust workspace (`locald-server`, `locald-cli`, `locald-core`).
- Implemented `LocaldConfig` schema.
- Implemented `locald-server` with Tokio runtime.
- Implemented `locald-cli` with Clap.
- Implemented IPC via Unix Domain Sockets (`ping` command).

## Phase 7: Persistence & State Recovery (2025-11-30)

**Goal**: Ensure `locald` persists state across restarts.

**Work Completed**:

- Implemented JSON state persistence in `~/.local/share/locald/state.json`.
- Added `StateManager` to handle loading/saving state.
- Implemented "Kill & Restart" strategy for process recovery to handle zombie processes.
- Refactored `locald-server/src/proxy.rs` to use `axum`, resolving dependency conflicts.
- Verified persistence manually (services restart automatically).

## Phase 8: Documentation Overhaul (2025-11-30)

**Goal**: Restructure documentation to serve specific user personas.

**Work Completed**:

- Defined "App Builder", "Power User", and "Contributor" personas in `docs/design/personas.md`.
- Restructured documentation sidebar into Guides, Concepts, Reference, and Internals.
- Created new guides: "Basic Configuration", "Architecture Overview", "Development Setup".
- Refined reference docs for Configuration and CLI.

## Phase 10: Multi-Service Dependencies (2025-12-01)

**Goal**: Support complex project structures where services depend on each other.

**Work Completed**:

- **Schema**: Added `depends_on` field to `ServiceConfig` in `locald-core`.
- **Logic**: Implemented topological sort (Kahn's algorithm) in `locald-server` to resolve startup order.
- **Process Manager**: Updated `start` logic to respect the resolved order.
- **Verification**: Verified startup order and cycle detection with unit tests and a manual test project.
