<!-- agent-template start -->

# Changelog

History of completed phases and key changes.

<!-- agent-template end -->

## M2.2: macOS Platform Support - Planning (2025-12-31)

**Goal**: Document requirements for `locald up` to work on macOS for exec services (native processes).

**Work Completed**:

- **RFC 0104**: Updated with comprehensive M2.2 requirements.
  - **P0 (Must Work)**: Daemon startup, exec service spawn, HTTP/HTTPS proxy, dashboard, log streaming, health checks, CLI commands, managed Postgres, project registry.
  - **P1 (Graceful Degradation)**: `/etc/hosts` automation (warn + manual sudo), `locald doctor` (macOS-specific), HTTPS cert trust (Keychain APIs).
  - **Linux-Only Exclusions**: `locald admin setup`, cgroup cleanup, container services, privileged ports.
- **MAP_AUDIT.md**: Marked M2.2 as "🚧 IN PROGRESS" with detailed requirements breakdown.
- **decisions.md**: Added Decision 069 documenting the progressive enhancement strategy.
- **Code Locations Identified**:
  - `locald-server/src/manager.rs` - Skip shim for hosts sync
  - `locald-utils/src/shim.rs` - Return None on macOS
  - `locald-utils/src/privileged.rs` - macOS doctor checks
  - `locald-cli/src/handlers.rs` - Error on `admin setup`
  - `locald-utils/src/process.rs` - Process group cleanup

**Status**: Planning complete. Implementation ready to begin.

## Phase 97: Libcontainer Shim (Fat Shim) (2025-12-13)

**Goal**: Remove the external `runc` dependency by executing OCI bundles via an embedded `libcontainer` runtime inside the privileged `locald-shim`.

**Work Completed**:

- **Runtime**: Removed remaining `runc` execution paths; container execution now goes through `locald-shim bundle run --bundle <PATH> --id <ID>`.
- **Docs/Coherence**: Updated manual + crate READMEs to prefer `bundle run` and to label the legacy `locald-shim bundle <bundle-path>` form as back-compat.
- **Verification**: `cargo xtask verify phase` passes (build, clippy, dashboard check, IPC verification).
- **Lint Hygiene (transition work)**: Fixed clippy failures surfaced by verification (doc hygiene/logging, blocking vs async process spawning in examples, removed unnecessary `async` wrappers).

## Phase 99e: Luminosity & State (2025-12-13)

**Goal**: Finish dashboard contrast/active-state polish and ensure the phase can transition cleanly under strict verification.

**Work Completed**:

- **Dashboard**: Improved contrast, active toolbar state, and hover affordances.
- **VMM (Bonus)**: Cleaned up `locald-vmm` VirtIO/MMIO/boot code to satisfy strict `clippy -D warnings` (docs, readable literals, `must_use`, and safer error propagation).
- **Roadmap (Bonus)**: Added Phase 102 and created RFC 0102 to capture the VMM reactor + networking direction.
- **Verification**: `cargo xtask verify phase` passes.

## Phase 99: Demo Setup & Polish (2025-12-12)

**Goal**: Polish the system to deliver a flawless "Vercel-class" demo for the CEO pitch, focusing on real-world complexity, self-hosting, and visual polish.

**Work Completed**:

- **Demo Repositories**:
  - Created `examples/color-system` and `examples/hydra` to simulate a realistic microservices environment.
  - Configured `locald.toml` for both projects with `exec` services.
- **Dashboard Polish (Phase 99c/d)**:
  - **Sidebar Alignment**: Fixed vertical alignment of icons and text in the sidebar.
  - **Visual Hierarchy**: Reduced gap between "Project" and "Services" sections.
  - **Log Readability**: Dimmed timestamps in logs to improve focus on content.
  - **SOLO Mode**: Styled the "SOLO" toast notification to be less intrusive.
- **Verification**: Verified that the demo repositories boot correctly and the dashboard looks polished.

## Phase 99: Structured Cgroup Hierarchy (Design) (2025-12-12)

**Goal**: Implement a strict, hierarchical Cgroup v2 structure ("Tree of Life") to ensure reliable lifecycle management and resource accounting, replacing `runc` with `libcontainer`.

**Work Completed**:

- **RFC 0099**: Designed and validated the "Anchor & Driver" strategy for Cgroup management.
  - **The Anchor**: Uses a Systemd Unit (`locald.slice`) with `Delegate=yes` when Systemd is present.
  - **The Driver**: Manually manages `/sys/fs/cgroup` when Systemd is absent.
  - **Validation**: Confirmed cross-platform compatibility (WSL2, Lima) via RFC 0061.
- **Manual**: Created `docs/manual/architecture/resource-management.md` as the source of truth.
- **Axiom**: Added [Axiom 14: Precise Abstractions](../design/axioms/architecture/14-precise-abstractions.md) to justify the move to low-level primitives.
- **Status**: RFC moved to Stage 3 (Recommended). Implementation pending.

## Phase 94: Builder Permissions & Context (2025-12-10)

**Goal**: Fix critical regressions in the builder and development loop to ensure stability.

**Work Completed**:

- **Builder Permission Fix**:
  - Diagnosed a regression where `locald-builder` failed to clean up `rootfs` due to root-owned artifacts created by `runc`.
  - Implemented a fallback mechanism in `locald-server` and `locald-builder` to use the privileged `locald-shim` for cleanup when standard deletion fails.
  - Verified that `locald` can now successfully clean up build directories containing root-owned files.
- **Dev Loop Stability**:
  - Diagnosed a "fork bomb" issue where `cargo run` updated the build timestamp on every run, triggering an infinite restart loop in `locald up`.
  - Fixed `locald-cli/build.rs` to remove the timestamp from `LOCALD_BUILD_VERSION`, ensuring stable builds.

## Phase 93: Proxy Lazy Loading (2025-12-10)

**Goal**: Improve perceived performance for slow-booting web services by serving an immediate "Loading..." page instead of hanging the connection.

**Work Completed**:

- **Proxy Logic**: Updated `locald-server/src/proxy.rs` to intercept HTML requests.
- **Lazy Loading Page**: Implemented a static HTML page that polls `/api/services/{name}` and auto-reloads when the service is healthy.
- **Timeout Handling**: Requests taking longer than 500ms now serve the loading page.
- **RFC**: Implemented [RFC 0093: Proxy Loading State](../rfcs/0093-proxy-loading-state.md).

## Phase 76: Ephemeral Containers (2025-12-10)

**Goal**: Implement `locald container run` to support ad-hoc container execution, validating the composition of `locald-oci`, `locald-shim`, and `locald-server`.

**Work Completed**:

- **OCI Library**: Implemented `OciRuntimeSpec` generation and `runc` wrapper in `locald-oci`.
- **Shim Integration**: Updated `locald-shim` to use the new runtime interface.
- **Server Logic**: Added `ContainerService` to `locald-server` to manage ephemeral containers.
- **CLI**: Added `locald container run` command supporting `-it` (interactive), `-d` (detached), and `-p` (ports).
- **Documentation**: Updated "Ad-Hoc Tasks" guide to document the new command and best practices.
- **RFC**: Advanced [RFC 0076: Ephemeral Containers](../rfcs/0076-ephemeral-containers.md) to Stage 3.

**Note**: The `runc` wrapper mentioned above is historical. Phase 97 (RFC 0098) removes the external `runc` dependency and executes OCI bundles via an embedded `libcontainer` runtime inside `locald-shim`.

## Phase 81: Dashboard Refinement (2025-12-09)

**Goal**: Elevate the dashboard from a passive viewer to a robust, interactive workspace.

**Work Completed**:

- **Frontend Architecture**: Implemented a new "Project-Centric" dashboard using SvelteKit.
  - **Sidebar**: Global navigation and status.
  - **ServiceGrid**: A grid view of services grouped by project.
  - **InspectorDrawer**: A slide-out drawer for detailed service inspection (logs, environment, config).
- **Asset Integration**:
  - Updated `locald-server/build.rs` to track `locald-dashboard` source files and rebuild if necessary.
  - Created `scripts/build-assets.sh` to standardize the asset build process.
  - Verified that `cargo install-locald` correctly bundles the new dashboard assets.
- **Resilience Testing (Phase 81.1)**:
  - Implemented "Glass Box" testing by exposing SSE connection state via `data-sse-connected` attribute.
  - Created `locald-dashboard-e2e/tests/resilience.spec.ts` to verify automatic reconnection after server restarts.
  - Updated test harness to support restarting the server on the same port.
  - Verified that the dashboard gracefully handles backend restarts without user intervention.

## Phase 79: Unified Service Trait (2025-12-09)

**Goal**: Unify all service types under a shared trait system (`ServiceController`, `ServiceFactory`) to allow for extensible service types and decouple the `ProcessManager` from specific implementations.

**Work Completed**:

- **Core Traits**: Defined `ServiceController` and `ServiceFactory` traits in `locald-core`.
- **Implementations**: Implemented controllers for:
  - `ExecController` (Host Process & Container)
  - `PostgresController` (Managed Postgres)
  - `DockerController` (Legacy Docker)
- **Manager Refactor**: Refactored `ProcessManager` to use the factory pattern, removing large `match` statements and enabling cleaner extensibility.
- **Verification**: Verified that all service types continue to function correctly under the new abstraction.
- **RFC**: Advanced [RFC 0079: Unified Service Trait](../rfcs/0079-unified-service-trait.md) to Stage 3.

## Phase 62 & 64: Host-First & Shared Utils (2025-12-08)

**Goal**: Complete the "Host-First" pivot and the "Shared Utility Crate" refactor, and prepare for the Documentation Restructure.

**Work Completed**:

- **Host-First Execution (Phase 64)**:
  - Verified Host-First default behavior and CNB Opt-In.
  - Updated documentation and created verification tests.
  - Advanced [RFC 0069: Host-First Execution Strategy](../rfcs/0069-host-first-execution.md) to Stage 3.
- **Shared Utility Crate (Phase 62)**:
  - Created `locald-utils` and migrated core modules (`fs`, `process`, `probe`, `cert`, `ipc`, `notify`).
  - Refactored `locald-server`, `locald-cli`, and `locald-builder` to use the new crate.
  - Enforced strict linting and documentation standards in `locald-utils`.
- **Preparation**:
  - **Shim Security**: Fixed permissions on `locald-shim` (4750) and updated setup scripts.
  - **Coherence**: Updated [Axiom 12](../design/axioms/architecture/12-source-of-truth.md) to clarify Runtime Contracts.
  - **Planning**: Created [RFC 0071: Documentation Restructure](../rfcs/0071-documentation-restructure.md) and updated the roadmap.

## Phase 64: Host-First Execution Pivot (2025-12-08)

**Goal**: Pivot the default execution strategy to "Host-First" (running processes directly on the host) to improve developer experience and reduce friction, making CNB an opt-in feature.

**Work Completed**:

- **Verified Host-First Default**: Confirmed that services without `build` or `image` config run directly on the host.
- **Verified CNB Opt-In**: Confirmed that adding `[service.build]` triggers the CNB lifecycle and runs in a container.
- **Documentation**: Updated "Getting Started" and "Configuration Reference" to reflect the new default and opt-in mechanism.
- **Tests**: Created `examples/host-first-test` and `examples/cnb-opt-in-test` to verify behavior.
- **RFC**: Advanced [RFC 0069: Host-First Execution Strategy](../rfcs/0069-host-first-execution.md) to Stage 3.

## Phase 61: Boot Feedback & Progress UI (2025-12-08)

**Goal**: Provide immediate, granular feedback during the `locald up` boot process to eliminate the "is it stuck?" anxiety.

**Work Completed**:

- **Progress Renderer**: Implemented a `ProgressRenderer` trait with a `TuiRenderer` implementation using `indicatif` to display a rich, multi-step progress bar.
- **IPC Events**: Updated `locald-server` to emit granular `ServiceEvent`s (Building, Starting, Healthy) over the IPC channel, enabling real-time status updates.
- **CLI Integration**: Updated `locald up` to subscribe to these events and render the progress UI, showing "Building...", "Starting...", and "Healthy" states for each service.
- **TUI Testing**: Added `rexpect` integration tests to verify the interactive progress UI in a pseudo-terminal environment.
- **RFC**: Implemented [RFC 0062: Boot Feedback & Progress UI](../rfcs/0062-boot-feedback.md).

## Phase 34.2: UI Polish & Host-First Refactor (2025-12-07)

**Goal**: Modernize the CLI UI and refactor the runtime for better maintainability.

**Work Completed**:

- **UI Polish (RFC 0070)**:
  - Replaced mixed output styles with a consistent "Task List" aesthetic using `cliclack`.
  - Implemented spinners for long-running operations (`locald up`).
  - Standardized success/failure messages with clear iconography.
- **Host-First Refactor (RFC 0069)**:
  - Refactored `ProcessRuntime` to extract common PTY and log streaming logic, reducing duplication between Host and CNB runtimes.
  - Verified Host-First execution works correctly for `color-system` (resolving the reported regression which was actually an application error).
- **Code Quality**:
  - Fixed numerous `clippy` lints across `locald-builder`, `locald-server`, and `locald-cli`.

## Phase 34.1: Stability Fixes & Output Polish (2025-12-08)

**Goal**: Address critical stability issues and improve output ergonomics.

**Work Completed**:

- **Symlink Handling**: Fixed a critical crash in `locald-builder` where broken symlinks in the project directory caused the daemon to panic with `os error 2`. Broken symlinks are now preserved as-is (broken) in the build context, preventing the crash while maintaining correct behavior for valid symlinks.
- **Regression Testing**: Added `locald-cli/tests/regression_symlinks.rs` to permanently verify symlink handling. Updated `docs/design/workflow-axioms.md` to mandate regression tests for all bug fixes.
- **Output Philosophy**: Refactored `locald-builder` to stream build output asynchronously via `tracing`. Raw buildpack output is now logged at `DEBUG` level (hidden by default), while high-level lifecycle phases are logged at `INFO`. This aligns with [Axiom 4: Respectful Output](../design/axioms/experience/04-output-philosophy.md).

## Phase 34 & 58: CNB Polish & Crash Protocol (2025-12-07)

**Goal**: Refine the CNB integration to respect the "Source of Truth" (OCI Config) and implement a robust Crash Protocol.

**Work Completed**:

- **Source of Truth (Axiom 12)**:
  - Refactored `locald-builder` to extract environment variables (`PATH`, etc.) from the builder image and cache them.
  - Refactored `locald-server` to inject these variables into the runtime container, removing hardcoded paths.
  - Documented [Axiom 12: The Source of Truth](../design/axioms/architecture/12-source-of-truth.md).
- **Development Loop (Axiom 13)**:
  - Formalized the "Split Lifecycle" strategy (Build = Snapshot, Run = Bind Mount) in [Axiom 13](../design/axioms/architecture/13-development-loop.md).
  - Updated [RFC 0059: Live Bind Mounts](../rfcs/0059-live-bind-mounts.md) to reflect this accepted design.
- **Crash Protocol**:
  - Updated [Axiom 4: Output Philosophy](../design/axioms/experience/04-output-philosophy.md) to define the "Crash Protocol".
  - Implemented `locald-cli/src/crash.rs` to capture panics and errors, writing full context to a log file (`.locald/crashes/`).
  - Updated `locald-cli` entry point to catch all errors.
- **UX Improvements**:
  - Proposed [RFC 0062: Boot Feedback & Progress UI](../rfcs/0062-boot-feedback.md).
  - Proposed [RFC 0063: E2E Testing Infrastructure](../rfcs/0063-e2e-testing.md).
  - Fixed `locald-builder` error reporting to include `stdout` (build logs) when `runc` fails, solving the "exit status 51" opacity issue.
  - Fixed `locald-cli` build script to correctly detect dependency changes and trigger auto-updates.

## Phase 34: Cloud Native Buildpacks (CNB) & Engineering Excellence (2025-12-06)

**Goal**: Integrate Cloud Native Buildpacks (`locald build`) and perform a Documentation-Driven Design Audit.

**Work Completed**:

- **Cloud Native Buildpacks (CNB)**:
  - **`locald-builder` Crate**: Implemented a Rust-native CNB platform that orchestrates the lifecycle (detect, analyze, build, export) without Docker.
  - **OCI Layout**: Implemented OCI Image Layout support for storing buildpack and app images on disk.
  - **CLI**: Added `locald build` command.
  - **Shim Runtime**: Used `locald-shim` to execute `runc` for privileged container operations in a secure way.
- **Engineering Excellence (Refactor & Document)**:
  - **Core Clarity**: Refactored `locald-core` IPC and Registry for better type safety and clarity.
  - **Decoupling**: Introduced `ServiceResolver` trait to decouple Proxy from Manager, enabling better testing.
  - **Documentation**: Added comprehensive `/// # Example` docstrings to public APIs.
  - **Testing**: Added persistence tests for `Registry` to prevent regressions.

## Phase 35: Shim Versioning & Upgrades (2025-12-04)

**Goal**: Ensure the privileged `locald-shim` is kept in sync with the `locald` daemon to prevent security vulnerabilities and protocol mismatches.

**Work Completed**:

- **Shim Self-Reporting**: Implemented `--shim-version` flag in `locald-shim` to report its own version (from `Cargo.toml`).
- **Build Integration**: Updated `locald-cli/build.rs` to read the shim version at build time and embed it as `LOCALD_EXPECTED_SHIM_VERSION`.
- **Runtime Verification**: Updated `locald-cli` to verify the installed shim version before attempting privileged operations.
- **User Experience**: Added clear error messages instructing users to run `sudo locald admin setup` if the shim is outdated.
- **RFC**: Created and implemented [RFC 0045: Shim Versioning](../rfcs/0045-shim-versioning.md).

## Phase 32: Sandbox Environments (2025-12-05)

**Goal**: Provide isolated environments for testing and CI/CD, allowing `locald` to run without affecting the global user configuration.

**Work Completed**:

- **Sandbox Mode**: Implemented `--sandbox <NAME>` flag to isolate `XDG_*` directories and the IPC socket.
- **Safety**: Enforced strict safety by panicking if `LOCALD_SOCKET` is set without the sandbox flag, preventing accidental environment leakage.
- **Testing**: Updated integration tests to use the sandbox mechanism.
- **Documentation**: Updated `AGENTS.md` to mandate sandbox usage for testing.

## Phase 26: Configuration & Registry (2025-12-04)

**Goal**: Manage complexity via structure, persistence, and cascading configuration.

**Work Completed**:

- **Global Configuration**:
  - Implemented `GlobalConfig` struct mapped to `~/.config/locald/config.toml`.
  - Implemented `ConfigLoader` with `Provenance` tracking to identify where config values originate (Global, EnvVar, etc.).
  - Added `locald config show` and `locald config show --provenance` commands.
- **Project Registry**:
  - Implemented a centralized `Registry` persisted to `~/.local/share/locald/registry.json`.
  - Added `locald registry` subcommands: `list`, `pin`, `unpin`, `clean`.
  - Integrated Registry with Daemon startup to support "Always Up" services (pinned projects).
- **Verification**: Verified the implementation with `verify-phase.sh` and manual testing.

## Phase 24: Dashboard Ergonomics & Navigation (2025-12-04)

**Goal**: Remove immediate friction and improve "at-a-glance" readability and navigation in the Dashboard.

**Work Completed**:

- **Global Controls**: Implemented "Stop All" and "Restart All" buttons in the Dashboard header, backed by new IPC commands.
- **Event Stream**: Implemented Server-Sent Events (SSE) at `/api/events` to push real-time updates (service status changes, logs) to the frontend, replacing polling.
- **Deep Linking**: Implemented URL synchronization so selecting a service updates the URL (`?service=name`), allowing for bookmarking and sharing.
- **Visual Polish**:
  - Added real-time connection status indicator.
  - Improved "All Services" view by stripping ANSI codes from logs to prevent rendering artifacts.
  - Refined the layout and visual feedback for actions.

## Phase 23: Advanced Service Configuration (2025-12-04)

**Goal**: Support a wider range of service types and configurations to match or exceed Foreman/Heroku capabilities.

**Work Completed**:

- **Service Types**: Support `type = "worker"` for non-network services (skip port assignment/probes).
- **Procfile Support**: Parse `Procfile` for drop-in compatibility; auto-generate config if missing.
- **Port Discovery**: Auto-detect ports for services that ignore `$PORT` (scan `/proc/net/tcp` or `lsof`).
- **Advanced Health Checks**:
  - Added `health_check` field to `ServiceConfig`. Supports string (command) or table (probe) format.
  - Implemented `spawn_http_health_monitor` using `reqwest` to poll HTTP endpoints.
  - Implemented `spawn_command_health_monitor` to run shell commands.
- **Foreman Parity**:
  - **.env Support**: Implemented automatic loading of `.env` files using `dotenvy`.
  - **Custom Signals**: Added `stop_signal` configuration to support `SIGINT`, `SIGQUIT`, etc.
- **Verification**: Verified with `examples/health-check-test`, `examples/env-test`, and `examples/signal-test`.

## Phase 21: UX Improvements (Web & CLI) (2025-12-04)

**Goal**: Improve the user experience of the built-in Web UI (dashboard) and the CLI/TUI to be more robust, readable, and interactive.

**Work Completed**:

- **Dashboard UI**: Rebuilt the dashboard using Svelte 5 and TypeScript. Implemented a responsive layout with a sidebar, service status indicators, and control buttons (Start/Stop/Restart).
- **Terminal Integration**: Integrated `xterm.js` into the dashboard for a high-fidelity, color-supporting terminal experience for viewing logs.
- **PTY Support**: Integrated `portable-pty` in the backend to spawn processes in pseudo-terminals, preserving ANSI colors and handling interactive applications better.
- **Secure Privilege Separation**: Implemented `locald-shim`, a small `setuid` binary that allows `locald` to bind to privileged ports (80/443) and inspect system ports without running the entire daemon as root.
- **CLI Status Table**: Upgraded `locald status` to use `comfy-table`, providing a cleaner, aligned, and colored output.
- **AI Integration**: Implemented `locald ai schema` and `locald ai context` commands to expose internal state and configuration schema to LLM agents.
- **Config Watcher**: Implemented a file watcher that automatically restarts the daemon when `locald.toml` changes.
- **Documentation**: Documented the new Security Architecture and deployed the updated docs to the embedded server.

## Phase 23: Advanced Service Configuration & Design Restructuring

**Date**: 2025-12-04

### Features

- **Advanced Config**: Added support for `.env` files, custom signal handling, and `procfile` parsing.
- **Design Manifesto**: Restructured the design documentation into a cohesive "Manifesto" with three pillars: Experience, Architecture, and Environment.
- **Vision**: Articulated the "Local Development Platform" vision in `docs/design/vision.md`.
- **Documentation Sync**: Implemented automated syncing of design docs to the `locald-docs` site.

### Key Changes

- Created `docs/design/vision.md`.
- Reorganized `docs/design/axioms/`.
- Added **Axiom 7: Ephemeral Runtime, Persistent Context**.
- Created design docs for **Dashboard Ergonomics** (Phase 24) and **Advanced Proxying**.

## Phase 22: Fresh Eyes Review & Documentation Update (2025-12-03)

**Goal**: Review the entire project state (CLI, Dashboard, Docs) and update documentation to reflect recent major changes (Builtin Services, UX Improvements).

**Work Completed**:

- **Documentation Overhaul**: Updated `locald-docs` to reflect Phase 20 (Builtin Services) and Phase 21 (UX Improvements).
  - Added `guides/builtin-services.md`.
  - Updated `reference/cli.md` and `reference/configuration.md`.
  - Updated `guides/getting-started.md` and `internals/architecture.md`.
  - Added "Database Integration" to `guides/common-patterns.mdx`.
- **Dashboard Polish**: Improved the Dashboard UX with better status indicators (Green/Yellow/Red) and tooltips for config sources.
- **CLI Review**: Verified CLI help text and error messages for consistency.
- **Verification**: Verified the entire system state and documentation accuracy.

## Phase 21: UX Improvements (Web & CLI) (2025-12-03)

**Goal**: Improve the user experience of the built-in Web UI (dashboard) and the CLI/TUI to be more robust, readable, and interactive.

**Work Completed**:

- **Dashboard UI**: Rebuilt the dashboard using Svelte 5 and TypeScript. Implemented a responsive layout with a sidebar, service status indicators, and control buttons (Start/Stop/Restart).
- **Terminal Integration**: Integrated `xterm.js` into the dashboard for a high-fidelity, color-supporting terminal experience for viewing logs.
- **PTY Support**: Integrated `portable-pty` in the backend to spawn processes in pseudo-terminals, preserving ANSI colors and handling interactive applications better.
- **CLI Status Table**: Upgraded `locald status` to use `comfy-table`, providing a cleaner, aligned, and colored output.
- **AI Integration**: Implemented `locald ai schema` and `locald ai context` commands to expose internal state and configuration schema to LLM agents.
- **Config Watcher**: Implemented a file watcher that automatically restarts the daemon when `locald.toml` changes.
- **Proxy Fix**: Fixed a routing issue where `locald.localhost` was serving embedded assets instead of the dynamic dashboard service.
- **Code Quality**: Fixed a large number of Clippy lints across the workspace, enforcing stricter code quality.

## Phase 20: Builtin Services (2025-12-03)

**Goal**: Provide "Heroku-style" managed data services (Postgres) that work out of the box without Docker, `mise`, or manual binary management.

**Work Completed**:

- **Managed Postgres**: Implemented `PostgresRunner` using `postgresql_embedded` to download, initialize, and run Postgres on demand.
- **Service Types**: Refactored `ServiceConfig` to support `type = "postgres"` alongside the default `exec`.
- **Dependency Injection**: Implemented environment variable substitution (e.g., `${services.db.url}`) to inject connection strings into dependent services.
- **CLI Updates**: Added `locald service add postgres <name>` and `locald service reset <name>` (to wipe data and restart).
- **UX Improvements**: Automated `.gitignore` updates to prevent checking in `.locald/` state directories.
- **Testing**: Added integration tests for Postgres startup and dependency injection.
- **Code Quality**: Fixed a large number of Clippy lints across the workspace, enforcing stricter code quality.

## Phase 15: Zero-Config SSL & Single Binary (2025-12-02)

**Goal**: Enable HTTPS support for `.localhost` domains using a pure Rust stack, simplify installation by merging binaries, and improve the "Try -> Save -> Run" workflow.

**Work Completed**:

- **Single Binary**: Merged `locald-server` into `locald-cli` (now just `locald`). Added `locald server` subcommand.
- **Zero-Config SSL**: Implemented on-the-fly certificate generation for `.localhost` domains using `rcgen` and `rustls`.
- **Trust Management**: Added `locald trust` to install a self-signed Root CA into the system trust store.
- **Seamless Updates**: Implemented auto-restart logic in `locald up` to handle binary updates without manual shutdown.
- **Graceful Shutdown**: Implemented robust shutdown protocol (SIGTERM -> Wait -> SIGKILL) using process groups.
- **Ad-Hoc Polish**: Improved `locald run` to use dynamic ports, `.localhost` domains, and accept multiple arguments.
- **Polish**: Implemented `GlobalConfig` for port binding control, strict port binding enforcement, and a fix for Vite HMR loops (rejecting WebSocket upgrades).
- **Verification**: Verified HTTPS support with `curl` and browser. Verified update and shutdown flows.

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

## Phase 19: Self-Hosted Documentation (2025-12-03)

**Goal**: Host the documentation directly from the `locald` binary to ensure it's always available and version-matched.

**Work Completed**:

- **Embedded Assets**: Used `rust-embed` to compile the static documentation site into the `locald` binary.
- **Build Automation**: Added a `build.rs` script to `locald-server` to automatically copy build artifacts from `locald-docs`.
- **Internal Routing**: Updated the internal proxy to route `docs.localhost` requests to the embedded assets.
- **Verification**: Verified that `http://docs.localhost:8081` serves the documentation correctly.

## Phase 18: Documentation Fresh Eyes (2025-12-03)

**Goal**: Review the documentation with "Fresh Eyes" to ensure it reflects the current state of the project, especially after the Phase 15 changes (Single Binary, SSL, `.localhost`).

**Work Completed**:

- **Getting Started**: Updated installation (Single Binary), added "Quick Run" (`locald run`), and "Enable HTTPS" (`locald trust`).
- **DNS & Domains**: Purged `.local` references in favor of `.localhost`. Documented Zero-Config SSL and Hosts File usage.
- **Reference**: Added `locald run`, `trust`, `server`, and `up` to CLI reference. Updated configuration defaults.
- **Internals**: Rewrote Architecture guide to reflect Single Binary model, SSL stack (`rcgen`, `rustls`), and Graceful Shutdown protocol.
- **Global Polish**: Purged outdated references to `locald-server` binary and `.local` domains across all documentation.
