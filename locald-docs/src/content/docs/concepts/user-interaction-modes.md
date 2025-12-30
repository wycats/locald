---
title: "Interaction Modes & Personas"
---


`locald` operates in several distinct modes depending on how the user interacts with it. Each mode corresponds to a specific **Persona** or mindset.

## 1. Daemon Mode (The System)

This is the "always-on" background process (`locald-server`).

- **Persona**: **The System** (Infrastructure, Background Service).
- **Responsibility**: Maintains the state of the world (registry, running processes, routing table).
- **Lifecycle**: Starts on system boot (or user login) and runs until explicitly stopped.
- **Interaction**: No direct user interaction. Communicates via IPC (Unix Socket).

## 2. Project Mode (The Developer)

This is when the user runs `locald` commands _inside_ a project repository.

- **Persona**: **The Developer** (Focused on the current task/project).
- **Context**: The current working directory determines the "Active Project".
- **Actions**: `locald start`, `locald stop`, `locald logs`.
- **Behavior**: The CLI reads `locald.toml`, resolves paths relative to the CWD, and sends instructions to the Daemon.

## 3. Global Mode (The Operator)

This is when the user runs `locald` commands _outside_ a specific project, or explicitly targets the system.

- **Persona**: **The Operator** (Focused on the overall system health).
- **Context**: No specific project.
- **Actions**: `locald status`, `locald prune`, `locald stop <name>`.
- **Behavior**: The CLI queries the Daemon for global state.

## 4. Interactive Mode (The Observer)

This provides a real-time view of the system.

- **Persona**: **The Observer** (Monitoring, Debugging, Insight).
- **Interfaces**:
  - **TUI**: `locald ui` or `locald monitor`. A terminal-based dashboard.
  - **Web UI**: `http://locald.localhost`. A browser-based dashboard.
- **Features**: Streaming logs, process status, start/stop controls.
- **Constraint**: Both interfaces consume the same API/Event Stream from the Daemon (Axiom 5).

