---
title: Architecture Overview
description: High-level architecture of locald.
---

`locald` is designed as a client-server system to ensure robustness and process independence.

## High-Level Diagram

```mermaid
graph TD
    CLI[locald-cli] -- IPC (Unix Socket) --> Server[locald-server]
    Server -- Spawns --> ServiceA[Service A (PID 101)]
    Server -- Spawns --> ServiceB[Service B (PID 102)]
    Browser -- HTTP :80 --> Server
    Server -- Proxy --> ServiceA
    Server -- Proxy --> ServiceB
```

## Components

### 1. `locald-cli`
*   **Role**: The user interface.
*   **Responsibility**: Parses arguments, sends commands to the server via IPC, and formats the response.
*   **Key Feature**: It is ephemeral. It runs, sends a command, and exits. It does *not* manage processes directly.

### 2. `locald-server`
*   **Role**: The daemon / supervisor.
*   **Responsibility**:
    *   **Process Management**: Spawns, monitors, and kills child processes.
    *   **State Management**: Persists the list of running services to `state.json`.
    *   **Proxying**: Listens on port 80 (or 8080) and routes requests to the correct service based on the `Host` header.
    *   **IPC Server**: Listens on `/tmp/locald.sock` for commands.

### 3. `locald-core`
*   **Role**: Shared library.
*   **Responsibility**: Defines shared data structures (configuration structs, IPC messages, state schema) used by both the CLI and Server.

## Key Flows

### Starting a Service
1.  User runs `locald start` in a project directory.
2.  CLI reads `locald.toml`.
3.  CLI sends `Register(service_config)` message to Server.
4.  Server assigns a free port.
5.  Server spawns the command with `PORT` env var.
6.  Server updates `state.json`.
7.  Server returns success to CLI.

### Request Routing
1.  Browser requests `http://my-app.local`.
2.  DNS resolves to `127.0.0.1` (via `/etc/hosts`).
3.  Request hits `locald-server` on port 80.
4.  Server looks up `my-app.local` in its internal routing table.
5.  Server proxies the request to the assigned port (e.g., `127.0.0.1:34123`).

