---
title: Architecture Overview
description: High-level architecture of locald.
---

`locald` is designed as a client-server system to ensure robustness and process independence, distributed as a single binary.

## High-Level Diagram

```mermaid
graph TD
    CLI[locald (Client)] -- IPC (Unix Socket) --> Server[locald (Daemon)]
    Server -- Invokes (setuid) --> Shim[locald-shim (Root)]
    Shim -- Binds --> Ports[Ports 80/443]
    Shim -- Modifies --> Trust[System Trust Store]
    Server -- Spawns --> ServiceA[Service A (PID 101)]
    Server -- Spawns --> ServiceB[Service B (PID 102)]
    Browser -- HTTPS :443 --> Server
    Server -- Proxy --> ServiceA
    Server -- Proxy --> ServiceB
```

## Components

### 1. `locald` (The Binary)

The `locald` binary contains both the client and server logic.

- When you run `locald up`, it acts as a **Client**.
- It spawns a background process (`locald server start`) which acts as the **Daemon**.

### 2. The Daemon (`locald-server`)

- **Role**: The supervisor and proxy.
- **Responsibility**:
  - **Process Management**: Spawns, monitors, and kills child processes. Uses `portable-pty` to provide a pseudo-terminal for better interactivity and color support.
  - **State Management**: Persists the list of running services to `state.json`.
  - **Config Watching**: Monitors `locald.toml` files for changes and automatically restarts services when configuration updates.
  - **Proxying**: Listens on port 80 (HTTP) and 443 (HTTPS) and routes requests.
  - **IPC Server**: Listens on `/tmp/locald.sock` for commands.

### 3. The Client (`locald-cli`)

- **Role**: The user interface.
- **Responsibility**: Parses arguments, sends commands to the server via IPC, and formats the response.
- **Key Feature**: It is ephemeral. It runs, sends a command, and exits.

### 4. The Shim (`locald-shim`)

- **Role**: The privileged gatekeeper.
- **Responsibility**:
  - **Privilege Separation**: Runs as `root` (via setuid) to perform restricted operations like binding to ports 80/443 and modifying the system trust store.
  - **Security**: Validates the caller and arguments before executing privileged actions.
  - **Versioning**: Ensures it matches the version of the `locald` daemon.
- **See Also**: [Shim Management](architecture/shim-management)

## Key Subsystems

### Zero-Config SSL

`locald` implements a "Pure Rust" SSL stack to support `.localhost` domains without external dependencies like `mkcert` or `openssl`.

1.  **Root CA**: `locald trust` generates a self-signed Root CA using `rcgen` and installs it into the system trust store (via `ca_injector`).
2.  **On-the-Fly Signing**: When a request comes in for `app.localhost`, the proxy (using `rustls`) dynamically generates a certificate for that domain and signs it with the Root CA in memory.
3.  **Result**: The browser sees a valid certificate for `app.localhost` signed by the trusted `locald` Root CA.

### Graceful Shutdown

To ensure data integrity, `locald` implements a robust shutdown protocol:

1.  **SIGTERM**: When `locald stop` is called, the daemon sends `SIGTERM` to all child processes.
2.  **Wait**: It waits for a configurable timeout (default 5s) for processes to exit.
3.  **SIGKILL**: If a process is still running after the timeout, it is forcibly killed with `SIGKILL`.
4.  **Process Groups**: `locald` uses process groups (`setsid`) to ensure that if a service spawns its own children, the entire tree is cleaned up.

## Key Flows

### Starting a Service

1.  User runs `locald up` in a project directory.
2.  Client checks if Daemon is running. If not, it spawns `locald server start` detached.
3.  Client resolves the absolute path and sends `Start { path }` message to Daemon.
4.  Daemon reads `locald.toml` from the provided path.
5.  **Dependency Resolution**: Daemon builds a dependency graph and performs a topological sort.
6.  **Sequential Startup**: For each service in order:
    - **Build (Optional)**: If configured, the Daemon triggers a CNB build (via `locald-builder`).
    - **Provisioning**: For builtin services (e.g., Postgres), the Daemon ensures binaries and data directories exist.
    - **Port Assignment**: Daemon assigns a free port.
    - **Execution**: Daemon spawns the command (or builtin process) with `PORT` env var.
    - **Health Check**: Daemon waits for the service to be healthy (via `sd_notify`, TCP, or HTTP).
    - **State Update**: Daemon updates internal state.
7.  Daemon persists state to `state.json`.
8.  Daemon returns success to Client.

### Request Routing

1.  Browser requests `https://my-app.localhost`.
2.  DNS resolves to `127.0.0.1`.
3.  Request hits `locald` on port 443.
4.  **TLS Handshake**: `locald` generates a cert for `my-app.localhost` and completes the handshake.
5.  Daemon looks up `my-app.localhost` in its internal routing table.
6.  Daemon proxies the request to the assigned port (e.g., `127.0.0.1:34123`).
