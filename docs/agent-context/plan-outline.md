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

- [ ] Support `depends_on` in `locald.toml`.
- [ ] Topological sort for startup order.
