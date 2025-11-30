---
title: Interaction Modes & Architecture
description: Understanding how locald works and who it is for.
---

`locald` is designed around the idea that different users interact with the system in different ways. We call these **Interaction Modes**.

## The 4 Modes

### 1. Daemon Mode (The System)

This is the "always-on" background process (`locald-server`).

- **Persona**: **The System**
- **Responsibility**: Maintains the state of the world (registry, running processes, routing table).
- **Lifecycle**: Starts on system boot (or user login) and runs until explicitly stopped.
- **Interaction**: No direct user interaction. Communicates via IPC (Unix Socket).

### 2. Project Mode (The Developer)

This is when you run `locald` commands _inside_ a project repository.

- **Persona**: **The Developer**
- **Focus**: The current task/project.
- **Context**: The current working directory determines the "Active Project".
- **Key Commands**:
  - `locald start`: Start the services defined in `locald.toml`.
  - `locald stop`: Stop the services for this project.
  - `locald logs`: View logs for this project.

### 3. Global Mode (The Operator)

This is when you run `locald` commands _outside_ a specific project, or explicitly target the system.

- **Persona**: **The Operator**
- **Focus**: Overall system health and resource usage.
- **Context**: No specific project.
- **Key Commands**:
  - `locald status`: See all running services across all projects.
  - `locald prune`: Clean up stopped processes.
  - `locald stop <service_name>`: Stop a specific service by name.

### 4. Interactive Mode (The Observer)

This provides a real-time view of the system.

- **Persona**: **The Observer**
- **Focus**: Monitoring, Debugging, Insight.
- **Interfaces**:
  - **TUI**: `locald monitor` (Coming Soon).
  - **Web UI**: `http://locald.local` (Coming Soon).

## Architecture

`locald` follows a client-server architecture to enable these modes.

### The Daemon (`locald-server`)

The daemon is a long-running process that:
1.  Manages child processes (your services).
2.  Assigns dynamic ports.
3.  Routes traffic (future).
4.  Exposes a Unix Domain Socket for control.

### The Client (`locald-cli`)

The `locald` command is a thin client. When you run a command:
1.  It reads the local configuration (`locald.toml`).
2.  It connects to the daemon via the Unix Socket.
3.  It sends a JSON command (e.g., "Start this service").
4.  It prints the response.

This separation ensures that your services keep running even if you close your terminal.
