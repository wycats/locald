# Project Plan Outline

This document is the high-level, chronological roadmap.

Conventions:

- `[x]` complete, `[ ]` planned / deferred.
- Phase numbers are historical identifiers (not necessarily contiguous).
- Links are relative to `docs/agent-context/`.

---

## Epoch 1: MVP (The Walking Skeleton)

**Goal**: A functional daemon that can register a local project, manage its process, and route traffic via `*.local`.

### Phase 1: Scaffolding & Daemon Basics

**Goal**: Set up the Rust workspace, implement the basic `locald-server` daemon, and a minimal `locald-cli` to ping it.

- [x] Initialize Rust workspace with `locald-server`, `locald-cli`, `locald-core`.
- [x] Implement basic Daemon loop (tokio).
- [x] Implement IPC (Unix Socket) for CLI-Daemon communication.
- [x] Create `locald.toml` configuration struct.

### Phase 2: Process Management (The Supervisor)

**Goal**: The daemon can spawn and manage child processes based on configuration.

- [x] Implement Process Manager (spawn, stop, restart).
- [x] Capture stdout/stderr.
- [x] Handle environment variables (PORT assignment).

### Phase 3: Documentation & Design Refinement

**Goal**: Establish a documentation site (Astro Starlight) to document the tool, serving the needs of our modes.

- [x] Firm up Interaction Modes & Personas.
- [x] Fresh Eyes review of Axioms & Implementation.
- [x] Set up Astro Starlight project.
- [x] Document existing features.

### Phase 4: Local DNS & Routing

**Goal**: Integrate `hostsfile` and a reverse proxy to route `app.local` to the managed port.

- [x] Implement `hostsfile` integration to manage `/etc/hosts`.
- [x] Implement basic HTTP proxy (Hyper/Axum) in the daemon.
- [x] Route requests based on Host header.

### Phase 5: Web UI & TUI Basics

**Goal**: A dashboard to see running apps and logs.

- [x] Serve a basic Web UI from the daemon.
- [x] Implement WebSocket for log streaming.
- [x] Add `locald logs` command to CLI.

## Epoch 2: Refinement & Robustness

**Goal**: Improve ergonomics, persistence, and multi-service support.

### Phase 6: Persona & Axiom Update

**Goal**: Review and update the project's personas and axioms based on the implementation experience of Epoch 1.

- [x] Review `docs/design/axioms.md`.
- [x] Review `docs/design/modes.md`.
- [x] Ensure alignment between code and philosophy.

### Phase 7: Persistence & State Recovery

**Goal**: The daemon should persist the list of registered projects/services so they can be restored after a restart.

- [x] Define a state file format (JSON/TOML).
- [x] Implement state saving on change/shutdown.
- [x] Implement state loading on startup.
- [x] Handle "zombie" processes (processes that are still running but the daemon forgot about them, or vice versa).

### Phase 8: Documentation Overhaul

**Goal**: Update docs to better serve specific user personas (App Builder, Power User, Contributor).

- [x] Create `docs/design/personas.md`.
- [x] Review and restructure existing documentation.
- [x] Create "How-to" guides for "Regular Joe".
- [x] Create Reference docs for "Power User".
- [x] Create Architecture docs for "Contributor".

### Phase 9: CLI Ergonomics & Interactive Mode

**Goal**: Improve the user experience of the CLI.

- [x] Better error messages and help text.
- [x] Interactive `locald init` to create `locald.toml`.
- [x] `locald monitor` (TUI) using `ratatui`?

### Phase 10: Multi-Service Dependencies

**Goal**: Support complex project structures.

- [x] Support `depends_on` in `locald.toml`.
- [x] Topological sort for startup order.

### Phase 11: Documentation & Persona Alignment

**Goal**: Ensure documentation fully serves the defined personas before adding more complexity.

- [x] Fresh Eyes review of existing docs.
- [x] Flesh out "App Builder" guides (Getting Started, Common Patterns).
- [x] Flesh out "Power User" reference (Configuration, CLI).
- [x] Flesh out "Contributor" docs (Architecture, Dev Setup).

## Epoch 3: Hybrid Development & Advanced Features

**Goal**: Support hybrid workflows (Docker + Local) and advanced networking.

### Phase 12: Docker Integration

**Goal**: Unified lifecycle for local apps and Docker containers.

- [x] Support `image` in `locald.toml`.
- [x] Manage Docker container lifecycle (run/stop).
- [x] Unified logging for containers.

### Phase 13: Smart Health Checks

**Goal**: Ensure services are ready before dependents start, using "Zero-Config" detection where possible.

- [x] Implement `sd_notify` listener for local processes.
- [x] Integrate Docker native health checks.
- [x] Implement TCP port probing as a fallback.
- [x] Update `depends_on` logic to wait for health.
- [x] Expose health source/status in UI/CLI.

### Phase 14: Dogfooding & Polish

**Goal**: Smooth out the rough edges before broader adoption. Focus on error messages, CLI output, and common workflow friction.

- [x] "Papercut" pass on CLI output (formatting, colors, clarity).
- [x] Verify "Zero-Config" flows work as expected in real scenarios.
- [x] Improve error handling for common misconfigurations.
- [x] Validate the "Happy Path" for a new user.

### Phase 15: Zero-Config SSL & Single Binary

**Goal**: Enable HTTPS support for `.localhost` domains using a pure Rust stack, simplify installation by merging binaries, and improve the "Try -> Save -> Run" workflow.

- [x] **Merge Binaries**: Refactor `locald-server` into a library and integrate it into `locald` CLI.
- [x] **CLI Workflow**: Implement `locald run` (with "add on exit" prompt) and `locald shutdown`.
- [x] **SSL Stack**: Implement "Pure Rust" CA generation (`rcgen`) and trust store injection (`ca_injector`).
- [x] **On-the-fly Signing**: Implement `ResolvesServerCert` in `rustls` to sign certificates dynamically.
- [x] **Default Domain**: Switch default domain from `.local` to `.localhost`.
- [x] **Verification**: Verify `.localhost` domain support in browsers.

### Phase 16: WebSocket Support

**Goal**: Enable full WebSocket support in the reverse proxy to support modern web apps and dev tools (HMR).

- [ ] **Upgrade Handling**: Implement WebSocket upgrade handling in `locald-server` proxy (handle `Connection: Upgrade` and `Upgrade: websocket` headers).
- [ ] **Connection Bridging**: Implement bidirectional copying of data between the upgraded client connection and the backend service.
- [ ] **Verification**: Verify with a WebSocket-heavy example app (e.g., a chat app or Vite HMR).

### Phase 17: Linting & Code Quality

**Goal**: Enforce stricter code quality standards to catch reliable errors without being overly pedantic.

- [ ] **Configure Clippy**: Add a `clippy.toml` or configure workspace-level lints for stricter checking (e.g., `warn` on `clippy::pedantic` but allow subjective ones).
- [ ] **Fix Lints**: Address existing lint warnings across the codebase.
- [ ] **CI Enforcement**: Ensure CI fails on lint warnings.

### Phase 18: Documentation Fresh Eyes

**Goal**: Review the documentation with "Fresh Eyes" to ensure it reflects the current state of the project, especially after the Phase 15 changes (Single Binary, SSL, `.localhost`).

- [x] **Review**: Walk through all documentation pages.
- [x] **Update**: Fix outdated references (e.g., `.local` -> `.localhost`, `locald-server` binary references).
- [x] **Polish**: Ensure the "Try -> Save -> Run" workflow is well-documented.
- [x] **Architecture**: Document the graceful shutdown protocol (SIGTERM -> Timeout -> SIGKILL) in the Contributor/Architecture section.

### Phase 19: Self-Hosted Documentation

**Goal**: Host the documentation directly from the `locald` binary to ensure it's always available and version-matched.

- [x] **Embedded Assets**: Used `rust-embed` to compile the static documentation site into the `locald` binary.
- [x] **Build Automation**: Added a `build.rs` script to `locald-server` to automatically copy build artifacts from `locald-docs`.
- [x] **Internal Routing**: Updated the internal proxy to route `docs.localhost` requests to the embedded assets.
- [x] **Verification**: Verified that `http://docs.localhost:8081` serves the documentation correctly.

### Phase 20: Builtin Services

**Goal**: Provide "Heroku-style" managed data services (Postgres, Redis) that work out of the box without Docker, `mise`, or manual binary management.
**Design Doc**: [../design/epoch-3-hybrid/managed-data-services.md](../design/epoch-3-hybrid/managed-data-services.md)

- [x] **Integration**: Integrate `postgresql_embedded` to download/run Postgres binaries.
- [x] **Service Types**: Implement "builtin" service type in `locald.toml`.
- [x] **Zero-Config**: Auto-assign ports, generate passwords, and manage data volumes.
- [x] **Injection**: Inject connection strings (e.g., `DATABASE_URL`) into dependent services.

### Phase 21: UX Improvements (Web & CLI)

**Goal**: Improve the user experience of the built-in Web UI (dashboard) and the CLI/TUI.
**Design Doc**: [../design/epoch-3-hybrid/ux-improvements.md](../design/epoch-3-hybrid/ux-improvements.md)

- [x] **Brainstorming**: Discuss ideas and flesh out the plan for UX improvements.
- [x] **Backend**: Switch to `portable-pty` for true terminal emulation (colors, progress bars).
- [x] **Web UI**: Rebuild dashboard with Svelte 5 + `xterm.js` for a robust, terminal-like experience.
- [x] **Service Controls**: Add Start/Stop/Restart/Reset buttons to the dashboard.
- [x] **CLI Tables**: Improve `locald status` output for narrow terminals (consider `nu-table` or similar).
- [x] **AI Usability**: Implement `locald ai context` and `locald ai schema` for LLM integration.
- [x] **Dashboard Alias**: Add `locald.localhost` as an alias for the dashboard (Already implemented).
- [x] **Polish**: Ensure the UI is responsive and accessible.
- [x] **Environment Clarity**: Show configuration sources in UI/CLI to help debug environment issues.
- [x] **Config Watcher**: Auto-restart daemon on global config change.
- [x] **Testability**: Ensure Web UI is testable (data-testid) and document how to test apps running under locald.
- [x] **Security**: Implement `locald-shim` for secure privilege separation.

### Phase 22: Fresh Eyes Review & Documentation Update

**Goal**: Review the entire project state (CLI, Dashboard, Docs) and update documentation to reflect recent major changes (Builtin Services, UX Improvements).

- [x] **Review**: Conduct a "Fresh Eyes" review of the CLI, Dashboard, and Documentation.
- [x] **Docs Update**: Update `locald-docs` to reflect Phase 20 (Builtin Services) and Phase 21 (UX Improvements).
- [x] **CLI Help**: Review and update CLI help text and error messages.
- [x] **Dashboard Polish**: Address any lingering UI/UX issues found during review.
- [x] **Axioms**: Review and update `docs/design/axioms.md` if necessary.

### Phase 23: Advanced Service Configuration

**Goal**: Support a wider range of service types and configurations to match or exceed Foreman/Heroku capabilities.
**Design Doc**: [../design/epoch-3-hybrid/advanced-service-config.md](../design/epoch-3-hybrid/advanced-service-config.md)

- [x] **Service Types**: Support `type = "worker"` for non-network services (skip port assignment/probes).
- [x] **Procfile Support**: Parse `Procfile` for drop-in compatibility; auto-generate config if missing.
- [x] **Port Discovery**: Auto-detect ports for services that ignore `$PORT` (scan `/proc/net/tcp` or `lsof`).
- [x] **Advanced Health Checks**: Add HTTP probe and Command execution health checks (e.g., `check_command` for DB readiness).
- [x] **Foreman Parity**: Support `.env` file loading, signal handling customization, and concurrency control.

### Phase 24: Dashboard Ergonomics & Navigation

**Goal**: Remove immediate friction and improve "at-a-glance" readability and navigation in the Dashboard.
**Axiom**: [Axiom 9: The Dashboard is a Workspace](../design/axioms/experience/02-dashboard-workspace.md)
**Design Doc**: [../design/epoch-3-hybrid/dashboard-ergonomics.md](../design/epoch-3-hybrid/dashboard-ergonomics.md)

- [x] **Global Controls**: "Restart All", "Stop All", and Connection Status.
- [x] **Event Stream**: Structured "All Services" view with sanitized logs and lifecycle events.
- [x] **Deep Linking**: Update URL on service selection (`?service=name`).
- [x] **Visual Polish**: Traffic light indicators, clickable links, refined layout.

### Phase 25: Roadmap & Design Organization

**Goal**: Organize the `docs/design` directory into structured phases in priority order. Ensure the roadmap is a good reflection of goals and the right order to achieve them in.

- [x] **Inventory**: Audit all files in `docs/design`.
- [x] **Prioritization**: Discuss and rank features/phases with the user.
- [x] **Restructure**: Organize design docs into a folder structure that mirrors the roadmap (e.g., `docs/design/epoch-3-hybrid/`).
- [x] **Plan Update**: Update `plan-outline.md` to reflect the agreed priorities.

### Phase 26: Configuration & Registry

**Goal**: Manage complexity via structure, persistence, and cascading configuration.
**RFC**: [docs/rfcs/0026-configuration-hierarchy.md](../rfcs/0026-configuration-hierarchy.md)
**Axiom**: [Axiom 10: Structured Service Hierarchy](../design/axioms/10-service-hierarchy.md)

- [x] **Global Config**: Implement `~/.config/locald/config.toml`.
- [x] **Registry**: Persist known projects and support "Always Up".

### Phase 27: Workspace & Constellations

**Goal**: Implement advanced configuration merging and service grouping.
**RFC**: [docs/rfcs/0026-configuration-hierarchy.md](../rfcs/0026-configuration-hierarchy.md)

- [ ] **Sorting & Grouping**: Stable sort, grouping by Constellation/Category.
- [ ] **Cascading Config**: Implement config inheritance (Global -> Context -> Project).

### Phase 28: Interactive Terminal & Theming

**Goal**: Elevate the Dashboard to an active controller with "Power User" features.

- [ ] **Interactive PTY**: Bidirectional terminal interaction in the browser.
- [ ] **Theming**: Light/Dark mode, curated themes.
- [ ] **Command Palette**: Quick actions via `Cmd+K`.

### Phase 29: Extensibility & Plugins

**Goal**: Allow users to extend `locald` and package custom distributions.
**RFC**: [docs/rfcs/0028-extensibility.md](../rfcs/0028-extensibility.md)

- [ ] **Plugin Mechanism**: Formal system for custom services (e.g., `locald.localhost` as a plugin).
- [ ] **Packaging**: Support `locald package /path/to/customizations` to create distributable bundles.
- [ ] **Distributions**: Allow distribution of "flavored" binaries/configs without recompilation.

### Phase 30: Installation & Update Experience

**Goal**: Streamline how users get, install, and update `locald` securely.
**RFC**: [docs/rfcs/0029-installation-and-updates.md](../rfcs/0029-installation-and-updates.md)

- [ ] **Distribution Channels**: Support `cargo binstall` and GitHub Releases.
- [ ] **Self-Upgrade**: Implement `locald selfupgrade` to fetch/apply updates and recycle the server.
- [ ] **Auto-Updates**: Implement opt-in auto-updates via a lightweight HEAD check (e.g., GH Pages).
- [ ] **Secure Installation**: Design a secure alternative to `curl | sh` (signatures, bootstrap binary).
- [ ] **Capability Preservation**: Preserve `cap_net_bind_service` across updates (wrapper binary or systemd integration).

### Phase 31: Engineering Excellence

**Goal**: Make `locald` rock-solid, easier to debug, and fully tested.
**RFC**: [docs/rfcs/0030-engineering-excellence.md](../rfcs/0030-engineering-excellence.md)

- [ ] **Error Handling**: Implement "Human-Readable" error strategy with context and full logging to `/tmp`.
- [ ] **Testing Strategy**: Implement `trycmd` for CLI testing and `assert_cmd` for integration tests.
- [ ] **Tested Docs**: Ensure documentation code samples are tested.

### Phase 32: Sandbox Environments

**Goal**: Provide isolated environments for testing and CI/CD, allowing `locald` to run without affecting the global user configuration.
**RFC**: [docs/rfcs/0031-sandbox-environments.md](../rfcs/0031-sandbox-environments.md)

- [x] **Sandbox Mode**: Implement `locald sandbox` command and `--sandbox` flag.
- [x] **Isolation**: Ensure sandbox instances use isolated config, data, and socket paths.
- [x] **CI Integration**: Document and verify usage in CI environments (GitHub Actions).

### Phase 33: Ad-Hoc Execution

**Goal**: Improve the workflow for running one-off commands and converting successful experiments into permanent configuration, aligning with Heroku terminology.
**RFC**: [docs/rfcs/0032-adhoc-execution.md](../rfcs/0032-adhoc-execution.md)

- [x] **Draft Mode**: Implement `locald try <command>` to experiment with commands and prompt to save.
- [x] **Task Mode**: Implement `locald run <service> <command>` to run tasks with injected environment variables.
- [x] **History & Recall**: Track `try` history and implement `locald add last`.

### Phase 35: Shim Versioning & Upgrades

**Goal**: Ensure the privileged `locald-shim` is kept in sync with the `locald` daemon to prevent security vulnerabilities and protocol mismatches.
**RFC**: [docs/rfcs/0045-shim-versioning.md](../rfcs/0045-shim-versioning.md)

- [x] **Shim Self-Reporting**: `locald-shim` reports its own version via `--shim-version`.
- [x] **Build Integration**: `locald-cli` embeds the expected shim version at build time.
- [x] **Runtime Verification**: `locald` verifies the shim version on startup and aborts with a clear error if mismatched.
- [x] **User Notification**: Instruct users to run `sudo locald admin setup` to update (or auto-fix in interactive mode).

### Phase 36: Hot Reloading & Configuration Watcher

**Goal**: Make the system responsive to configuration changes by automatically restarting services when `locald.toml` is modified, and ensuring manual restarts always pick up the latest config.
**RFC**: [docs/rfcs/0050-hot-reloading.md](../rfcs/0050-hot-reloading.md)

- [ ] **Fix Manual Restart**: Ensure `locald restart` reloads config from disk.
- [ ] **Project Config Watcher**: Implement a watcher for registered project `locald.toml` files.
- [ ] **Hot Reload Logic**: Wire up the watcher to the service restart logic.
- [ ] **Debouncing**: Add debouncing to the watcher.

### Phase 37: Port Mismatch Detection

**Goal**: Help users debug "it works but locald thinks it failed" scenarios by detecting when an app listens on the wrong port.
**RFC**: [docs/rfcs/0051-port-mismatch-detection.md](../rfcs/0051-port-mismatch-detection.md)

- [ ] **Refactor Discovery**: Move `locald-server/src/discovery.rs` to `locald-core`.
- [ ] **Implement Monitor**: Create a `PortMonitor` struct.
- [ ] **Integrate into `try`**: Add mismatch warnings to `locald try`.

### Phase 79: Unified Service Trait

**Goal**: Unify all service types under a shared trait system (`ServiceController`, `ServiceFactory`) to allow for extensible service types and decouple the `ProcessManager` from specific implementations.
**RFC**: [docs/rfcs/0079-unified-service-trait.md](../rfcs/0079-unified-service-trait.md)

- [x] **Core Traits**: Define `ServiceController` and `ServiceFactory` in `locald-core`.
- [x] **Implementations**: Implement controllers for Postgres, Exec, and Docker.
- [x] **Manager Refactor**: Update `ProcessManager` to use the factory pattern.
- [x] **Site Service**: Implement `locald service add site` for static sites.

### Phase 81: Dashboard Refinement

**Goal**: Elevate the dashboard from a passive viewer to a robust, interactive workspace by addressing log interaction, self-representation, and global controls.
**RFC**: [docs/rfcs/0081-dashboard-refinement.md](../rfcs/0081-dashboard-refinement.md)

- [x] **Frontend Architecture**: Implement new Sidebar, Grid, and Inspector layouts.
- [x] **Asset Integration**: Automate dashboard build and embedding in `locald` binary.
- [x] **Real-Time Foundation**: Verify `ServiceUpdate` broadcasting (SSE) integration.
- [x] **Log Interaction**: Implement explicit "Copy" controls and "Smart Folding" for logs.
- [x] **Global Controls**: Add "Restart All" and "Stop All" buttons.
- [x] **Polish**: Address final UI tweaks (Icon alignment, Sidebar scrolling).

### Phase 93: Proxy Lazy Loading

**Goal**: Improve perceived performance for slow-booting web services by serving a "Loading..." page while the service starts.
**RFC**: [docs/rfcs/0093-proxy-lazy-loading.md](../rfcs/0093-proxy-lazy-loading.md)

- [x] **RFC 0093**: Create and implement.
- [x] **Implementation**: Intercept HTML requests and serve loading page.
- [x] **Verification**: Verify with `examples/shop-frontend`.

### Phase 94: Builder Permissions & Context

**Goal**: Fix regression in builder permissions (rootfs cleanup) and stabilize the dev loop.

- [x] **Fix**: `cargo install-locald` alias.
- [x] **Bug**: Builder Permission Denied (Regression).
- [x] **Bug**: Dev Loop Instability (Fork Bomb).

### Phase 96: Shim Architecture (Leaf Node Axiom)

**Goal**: Structurally prevent recursive execution loops ("fork bombs") by enforcing that `locald-shim` never executes `locald`.
**RFC**: [docs/rfcs/0096-shim-execution-safety.md](../rfcs/0096-shim-execution-safety.md)

- [x] **Axiom**: Define the "Leaf Node Axiom".
- [x] **FD Passing**: Implement `bind` command in shim using `SCM_RIGHTS`.
- [x] **Refactor**: Remove `server start` and recursive logic from shim.
- [x] **Compliance**: Add static analysis tests to forbid `Command::new("locald")`.

---

## Epoch 4: The Build Era

**Goal**: Integrate modern container build workflows.

### Phase 34: Cloud Native Buildpacks (CNB)

**Goal**: A modern, container-native build workflow that replaces `pack` and integrates with DevContainers.
**RFC**: [docs/rfcs/0043-cnb-integration.md](../rfcs/0043-cnb-integration.md)

- [x] **Research**: Investigate `libcnb` and the CNB lifecycle.
- [x] **Platform**: Implement a basic CNB platform in Rust (or wrap `pack`).
- [x] **Integration**: Add `locald build` command.
- [x] **Environment**: Parse OCI Image Config to derive default `Env` (PATH) instead of hardcoding.
- [x] **Crash Protocol**: Implement "Black Box" logging for panics and errors.
- [x] **Source of Truth**: Ensure `locald-builder` and `locald-server` respect OCI config.

### Phase 38: E2E Testing Infrastructure

**Goal**: Implement a robust End-to-End test suite that verifies the full stack (CLI -> Daemon -> Shim -> Container) in a production-like environment.
**RFC**: [docs/rfcs/0063-e2e-testing.md](../rfcs/0063-e2e-testing.md)

- [x] **Test Harness**: Create a harness that builds release binaries and sets up the shim correctly.
- [x] **E2E Suite**: Implement tests for `locald up`, `locald run`, and CNB builds.
- [x] **CI Integration**: Run E2E tests in GitHub Actions.

### Phase 58: Documentation-Driven Design (DDD) Audit

**Goal**: Execute a Documentation-Driven Design (DDD) Campaign to audit, refactor, and document the codebase using specific personas.
**RFC**: [docs/rfcs/0058-documentation-driven-design-audit.md](../rfcs/0058-documentation-driven-design-audit.md)

- [ ] **Audit**: Audit code using "New Hire", "Security Auditor", "Test Engineer", and "SRE" personas.
- [ ] **Refactor**: Remove friction points identified during the audit.
- [ ] **Document**: Write high-quality Rustdoc and Doctests.

### Phase 59: Systematic Investigation Protocol

**Goal**: Establish a scientific method for debugging complex failures to avoid "mashing".
**RFC**: [docs/rfcs/0059-systematic-investigation-protocol.md](../rfcs/0059-systematic-investigation-protocol.md)

- [x] **Define Protocol**: Create the RFC defining the 5-step investigation process.
- [ ] **Adopt**: Use this protocol to solve the `runc` namespace issue in Phase 34.

### Phase 60: Manual Synchronization

**Goal**: Ensure the `docs/manual` fully reflects the architectural decisions and features defined in the RFCs, particularly for recent phases (CNB, Shim, Investigation).

- [ ] **Audit**: Compare `docs/rfcs` (Stage 3+) against `docs/manual`.
- [ ] **Update**: Consolidate RFC content into the manual.
  - [x] Create `docs/manual/features/execution-modes.md` (Host vs Container).
  - [x] Update `docs/manual/features/builds.md` to reflect Opt-In status.
  - [x] Update `docs/manual/features/process-types.md` to prioritize Host execution.
- [ ] **Verify**: Ensure the manual is the single source of truth for "how it works now".

### Phase 61: Boot Feedback & Progress UI

**Goal**: Provide immediate, rich feedback during the boot process (`locald up`) so users aren't left staring at a blank screen or opaque errors.
**RFC**: [docs/rfcs/0062-boot-feedback.md](../rfcs/0062-boot-feedback.md)

- [x] **TUI Framework**: Integrate `indicatif` or similar for progress bars.
- [x] **Step Tracking**: Instrument the boot process to report steps ("Building", "Starting", "Waiting").
- [x] **Log Streaming**: Stream logs for failing steps immediately.

### Phase 62: Shared Utility Crate

**Goal**: Extract common utilities into a shared crate to improve code reuse and security.
**RFC**: [docs/rfcs/0067-strawman-utility-crate.md](../rfcs/0067-strawman-utility-crate.md)

- [x] **Scaffold**: Create `locald-utils` crate.
- [x] **Migrate**: Move `fs`, `process`, `probe`, `cert`, `ipc`, `notify` modules.
- [x] **Refactor**: Update consumers (`locald-server`, `locald-cli`, `locald-builder`) to use the new crate.

### Phase 63: Dependency Hygiene

**Goal**: Ensure dependencies are kept up to date to maintain security and performance.
**RFC**: [docs/rfcs/0068-dependency-management.md](../rfcs/0068-dependency-management.md)

- [x] **Policy**: Establish a dependency management policy.
- [x] **Tooling**: Create `scripts/update-deps.sh` to automate updates.
- [x] **Protocol**: Update `AGENTS.md` to enforce regular updates.

### Phase 64: Host-First Execution Pivot

**Goal**: Pivot the default execution strategy to "Host-First" (running processes directly on the host) to improve developer experience and reduce friction, making CNB an opt-in feature.
**RFC**: [docs/rfcs/0069-host-first-execution.md](../rfcs/0069-host-first-execution.md)

- [x] **Configuration**: Update `locald.toml` schema to support optional `[service.build]`.
- [x] **Runtime Split**: Refactor `ProcessManager` to default to Host Runtime.
- [x] **Opt-In CNB**: Gate existing CNB logic behind the `build` configuration.
- [x] **Verification**: Verify Host-First and CNB Opt-In behavior with examples.

### Phase 65: CNB Library Extraction

**Goal**: Extract generic CNB orchestration logic into a standalone `cnb-client` library to decouple it from `locald` specifics and enable reuse.
**RFC**: [docs/rfcs/0068-cnb-library.md](../rfcs/0068-cnb-library.md)

- [ ] **Scaffold**: Create `cnb-client` crate.
- [ ] **Migrate**: Move `oci_layout` and `runtime_spec` logic.
- [ ] **Refactor**: Update `locald-builder` to use `cnb-client`.

### Phase 66: Rust CNB Launcher (Research)

**Goal**: Create a clean-sheet Rust implementation of the CNB `launcher` binary to improve performance and safety.
**RFC**: [docs/rfcs/0069-rust-cnb-launcher.md](../rfcs/0069-rust-cnb-launcher.md)

- [ ] **Prototype**: Create `cnb-launcher` crate and implement basic config parsing.
- [ ] **Environment**: Implement env var merging logic.
- [ ] **Execution**: Implement `execve` strategy.

### Phase 67: Documentation Restructure

**Goal**: Transform `docs.localhost` into a coherent, persona-driven documentation site that serves as the single entry point for all users, moving away from a "random assortment of files".
**Plan**: [docs-restructure-plan.md](docs-restructure-plan.md)

- [x] **IA Overhaul**: Restructure `locald-docs` to match Personas (Guides, Reference, Internals).
- [x] **Landing Page**: Redesign `index.mdx` to provide clear pathways.
- [x] **Migration**: Move and refine content from `docs/manual` and `docs/design`.
- [x] **Axiom Placement**: Move Axioms/Vision to "Internals" section.
- [x] **Refinement**: Polish Reference and Internals content to match current implementation.

### Phase 68: Runc Container Runtime

**Goal**: Adopt `runc` as the primary container runtime, enabling a daemonless architecture and better cross-platform support (via WSL/Lima).
**RFC**: [docs/rfcs/0075-runc-container-runtime.md](../rfcs/0075-runc-container-runtime.md)

- [x] **OCI Crate**: Extract OCI logic into `locald-oci` crate.
- [x] **Refactor**: Consolidate `start_container` and `start_cnb_container` in `locald-server`.
- [x] **Bundle Source**: Implement `BundleSource` trait in `locald-builder`.
- [x] **Deprecate**: Mark `DockerRuntime` as deprecated.

### Phase 76: Ephemeral Containers

**Goal**: Implement `locald run <image>` to support ad-hoc container execution, validating the composition of `locald-oci`, `locald-shim`, and `locald-server`.
**RFC**: [docs/rfcs/0076-ephemeral-containers.md](../rfcs/0076-ephemeral-containers.md)

- [x] **Spec Generation**: Implement robust `OciImageConfig` -> `OciRuntimeSpec` conversion in `locald-oci`.
- [x] **Runtime Interface**: Implement `locald_oci::runtime::run` wrapper.
- [x] **Shim Integration**: Update `locald-shim` to use the new runtime interface.
- [x] **Server Logic**: Add `ContainerService` to `locald-server`.
- [x] **CLI Command**: Add `locald run` command.

---

## Epoch 5: The Perfect Demo

**Goal**: Polish the system to deliver a flawless "Vercel-class" demo for the CEO pitch, focusing on real-world complexity, self-hosting, and visual polish.

### Phase 97: Libcontainer Shim (Fat Shim)

**Goal**: Replace the fragile `runc` dependency with an embedded `libcontainer` runtime to ensure reliable, self-contained execution for the demo.
**RFC**: [docs/rfcs/0098-libcontainer-execution-strategy.md](../rfcs/0098-libcontainer-execution-strategy.md)
**Status**: Completed

- [x] **Refactor Shim**: Update `locald-shim` to link `libcontainer` and execute OCI bundles directly.
- [x] **Update Daemon**: Update `locald-server` to generate OCI bundles and invoke the new shim.
- [x] **Verify**: Ensure `locald run` works reliably without external `runc`.

### Phase 98: Postgres Port Fix

**Goal**: Fix the hardcoded port issue in the Postgres service factory to allow dynamic port assignment and prevent conflicts.

- [x] **Diagnosis**: Confirm `PostgresFactory` hardcodes port 5432 when not specified.
- [x] **Fix**: Update `PostgresFactory` to request a dynamic port if none is configured.
- [x] **Verification**: Verify multiple Postgres services can run simultaneously without port conflicts.

### Phase 99a: Demo Repository Setup

**Goal**: Create the `examples/color-system` and `examples/hydra` directories with the specific `locald.toml` configurations mentioned in the pitch.

- [x] **Create Repos**: Set up `examples/color-system` and `examples/hydra`.
- [x] **Configure**: Add `locald.toml` with `exec` services (mocked with `python3 -m http.server` or similar).
- [x] **Verify**: Ensure they boot and are accessible via `*.localhost`.

### Phase 99b: Dashboard Polish

**Goal**: Ensure the dashboard displays the "Vercel" aesthetic and functions perfectly for the demo.

- [x] **Visual Check**: Verify layout, fonts, and colors match the desired aesthetic.
- [x] **Link Verification**: Ensure "Open App" links use `https://` and work correctly.
- [x] **Data Accuracy**: Verify service status and logs are accurate.
- [x] **Tiled Terminals**: Implement "Deck" view with tiled terminals for pinned services.
- [x] **Visual Polish**: Apply "Vercel-class" styling (Zinc scale, subtle borders, refined headers).

### Phase 99e: Luminosity & State

**Goal**: Refine the dashboard UI to improve contrast, accessibility, and state visibility, specifically focusing on the "Active" toolbar and global contrast tuning.

- [x] **Active Toolbar**: Make toolbar controls persistent and styled when active.
- [x] **Global Contrast**: Brighten sidebar items, headers, badges, and timestamps.
- [x] **Hover State**: Enhance row hover background.

### Phase 100: Meta-Hosting Verification

**Goal**: Verify `locald` can run the `dotlocal` project itself (which includes `locald-docs`, `locald-dashboard`, and `postgres`) alongside the demo repos.

- [x] **Self-Host**: Run `locald up` in the `dotlocal` root.
- [x] **Conflict Check**: Ensure no port conflicts with system services or other `locald` instances.
- [x] **Full Stack**: Verify Docs, Dashboard, and Postgres are all functional.

### Phase 101: System Plane & Unified Pinning

**Goal**: Implement a dedicated "System Plane" for daemon observability and simplify the dashboard interaction model by unifying "Solo" and "Pin" modes.
**RFC**: [docs/rfcs/0100-system-plane-and-unified-pinning.md](../rfcs/0100-system-plane-and-unified-pinning.md)

- [x] **RFC 0100**: Create and implement.
- [x] **System Plane**: Implement "System Normal" footer to toggle Daemon Control Center.
- [x] **Unified Pinning**: Deprecate "Solo Mode" in favor of a single "Pin" state.
- [x] **Daemon Logs**: Broadcast `locald` internal logs to the frontend.
- [x] **Dev Mode**: Add `dashboard` service to `locald.toml` for meta-hosting.

### Phase 102: VMM Maturity & Networking

**Goal**: Evolve `locald-vmm` from a bootable kernel demo into a foundation for real workloads by introducing an event-driven architecture, adding networking, and standardizing device implementations.
**RFC**: [docs/rfcs/0102-vmm-maturity-and-networking.md](../rfcs/0102-vmm-maturity-and-networking.md)

- [ ] **Event Loop**: Introduce a reactor model (epoll/mio/event-manager) with a VCPU thread + device I/O loop.
- [ ] **Async I/O**: Move virtio-block data path off the VCPU thread (thread pool or io_uring) and signal completion via eventfd/irq.
- [ ] **Virtio-Net**: Add a minimal `virtio-net` device (TAP) and integrate with the reactor.
- [ ] **Library Decision**: Evaluate adopting `dbs-virtio-devices` (or similar) once the reactor exists.
