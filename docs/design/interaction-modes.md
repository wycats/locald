# Interaction Modes

`locald` operates in several distinct modes depending on how the user interacts with it.

## 1. Daemon Mode (The Supervisor)

This is the "always-on" background process (`locald-server`).

- **Responsibility**: Maintains the state of the world (registry, running processes, routing table).
- **Lifecycle**: Starts on system boot (or user login) and runs until explicitly stopped.
- **Interaction**: No direct user interaction. Communicates via IPC (Unix Socket).

## 2. Project Mode (The Developer Context)

This is when the user runs `locald` commands _inside_ a project repository.

- **Context**: The current working directory determines the "Active Project".
- **Actions**: `locald up`, `locald down`, `locald logs`.
- **Behavior**: The CLI reads `locald.toml`, resolves paths relative to the CWD, and sends instructions to the Daemon.

## 3. Global Mode (The System Context)

This is when the user runs `locald` commands _outside_ a specific project, or explicitly targets the system.

- **Context**: No specific project.
- **Actions**: `locald list`, `locald status`, `locald prune`.
- **Behavior**: The CLI queries the Daemon for global state.

## 4. Interactive Mode (The Dashboard)

This provides a real-time view of the system.

- **Interfaces**:
  - **TUI**: `locald ui` or `locald monitor`. A terminal-based dashboard.
  - **Web UI**: `http://locald.local` (or similar). A browser-based dashboard.
- **Features**: Streaming logs, process status, start/stop controls.
- **Constraint**: Both interfaces consume the same API/Event Stream from the Daemon (Axiom 5).
