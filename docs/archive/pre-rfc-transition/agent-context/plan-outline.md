# Project Plan Outline

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

## Epoch 2: Refinement & Robustness

**Goal**: Improve ergonomics, persistence, and multi-service support.

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
**Design Doc**: [docs/design/epoch-3-hybrid/managed-data-services.md](docs/design/epoch-3-hybrid/managed-data-services.md)

- [x] **Integration**: Integrate `postgresql_embedded` to download/run Postgres binaries.
- [x] **Service Types**: Implement "builtin" service type in `locald.toml`.
- [x] **Zero-Config**: Auto-assign ports, generate passwords, and manage data volumes.
- [x] **Injection**: Inject connection strings (e.g., `DATABASE_URL`) into dependent services.

### Phase 21: UX Improvements (Web & CLI)

**Goal**: Improve the user experience of the built-in Web UI (dashboard) and the CLI/TUI.
**Design Doc**: [docs/design/epoch-3-hybrid/ux-improvements.md](docs/design/epoch-3-hybrid/ux-improvements.md)

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
**Design Doc**: [docs/design/epoch-3-hybrid/advanced-service-config.md](docs/design/epoch-3-hybrid/advanced-service-config.md)

- [x] **Service Types**: Support `type = "worker"` for non-network services (skip port assignment/probes).
- [x] **Procfile Support**: Parse `Procfile` for drop-in compatibility; auto-generate config if missing.
- [x] **Port Discovery**: Auto-detect ports for services that ignore `$PORT` (scan `/proc/net/tcp` or `lsof`).
- [x] **Advanced Health Checks**: Add HTTP probe and Command execution health checks (e.g., `check_command` for DB readiness).
- [x] **Foreman Parity**: Support `.env` file loading, signal handling customization, and concurrency control.

### Phase 24: Dashboard Ergonomics & Navigation

**Goal**: Remove immediate friction and improve "at-a-glance" readability and navigation in the Dashboard.
**Axiom**: [Axiom 9: The Dashboard is a Workspace](docs/design/axioms/experience/02-dashboard-workspace.md)
**Design Doc**: [docs/design/epoch-3-hybrid/dashboard-ergonomics.md](docs/design/epoch-3-hybrid/dashboard-ergonomics.md)

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
**Axiom**: [Axiom 10: Structured Service Hierarchy](docs/design/axioms/10-service-hierarchy.md)

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

- [ ] **Draft Mode**: Implement `locald try <command>` to experiment with commands and prompt to save.
- [ ] **Task Mode**: Implement `locald run <service> <command>` to run tasks with injected environment variables.
- [ ] **History & Recall**: Track `try` history and implement `locald add last`.

## Epoch 4: The Build Era

**Goal**: Integrate modern container build workflows.

### Phase 34: Cloud Native Buildpacks (CNB)

**Goal**: A modern, container-native build workflow that replaces `pack` and integrates with DevContainers.
**RFC**: [docs/rfcs/0033-cnb-integration.md](../rfcs/0033-cnb-integration.md)

- [ ] **CNB Library**: Interface with `libcnb` crates.
- [ ] **Lifecycle**: Implement detect/build/export lifecycle in Rust.
- [ ] **DevContainers**: Use resulting OCI images for DevContainers or direct execution.
