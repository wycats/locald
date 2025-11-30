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

- [ ] Implement Process Manager (spawn, stop, restart).
- [ ] Capture stdout/stderr.
- [ ] Handle environment variables (PORT assignment).

### Phase 3: Local DNS & Routing

**Goal**: Integrate `hostsfile` and a reverse proxy to route `app.local` to the managed port.

- [ ] Implement `hostsfile` integration to manage `/etc/hosts`.
- [ ] Implement basic HTTP proxy (Hyper/Axum) in the daemon.
- [ ] Route requests based on Host header.

### Phase 4: Web UI & TUI Basics

**Goal**: A dashboard to see running apps and logs.

- [ ] Serve a basic Web UI from the daemon.
- [ ] Implement WebSocket for log streaming.
- [ ] Add `locald logs` command to CLI.

## Epoch 2: Refinement & Robustness

**Goal**: Improve ergonomics, persistence, and multi-service support.
