# Architecture: Core

This document describes the core architecture of `locald`, including the daemon lifecycle, IPC, and process supervision.

## Related architecture docs

- [Plugins](plugins.md): WASM component plugins (detect/apply), plan validation, and CLI tooling.

## 1. Daemon + CLI Split

The system is composed of two logical components (implemented today as a single `locald` binary):

- **Daemon ("server mode")**: A long-running background process that manages the state of services, logs, and configuration.
  - Implemented by the `locald-server` crate.
  - Started via `locald server start`.
- **CLI ("client mode")**: Ephemeral commands that send requests to the daemon and display results.
  - Implemented by the `locald-cli` crate.

### Daemonization

Most user-facing commands (e.g. `locald up`) ensure the daemon is running:

1. Attempt an IPC `Ping`.
2. If the daemon is not reachable, spawn a new daemon by running `setsid locald server start`.

This "shell-level" detachment is the current default bootstrap mechanism.

The server implementation also contains an optional self-daemonization mode (PID file + stdout/stderr redirection), but the current CLI entrypoint runs the server in the foreground and relies on `setsid` for backgrounding.

Current development log behavior:

- When spawned by the CLI, daemon stdout/stderr are redirected to `/tmp/locald.log`.
- When run directly in the foreground (`locald server start`), logs go to the invoking terminal.

## 2. Inter-Process Communication (IPC)

Communication between the CLI and the Daemon happens over **Unix Domain Sockets**.

- **Socket Path**:
  - Default: `/tmp/locald.sock`
  - Sandboxed runs: `LOCALD_SOCKET` is set by the CLI to a sandbox-specific path (for example `~/.local/share/locald/sandboxes/<name>/locald.sock`).
    - `LOCALD_SOCKET` is only permitted when `LOCALD_SANDBOX_ACTIVE=1`.
- **Protocol**:
  - One JSON request per connection.
  - For streaming responses (boot events, logs, container output), the server writes a stream of newline-delimited JSON values.
  - Non-streaming responses are a single JSON value.
- **Security**: The Unix socket is a filesystem object; access is mediated by filesystem permissions on the socket path.

## 3. Process Supervision

The daemon acts as a supervisor for child processes (services).

### Lifecycle

- **Spawn**: Services are spawned as child processes.
- **Environment**: The daemon injects environment variables (e.g., `PORT`, `DATABASE_URL`) into the child process.
- **Monitoring**: The daemon monitors the exit status of child processes.
- **Recovery**:
  - **Restart**: If a service crashes, the daemon can restart it (configurable).
  - **Restore Cleanup**: On startup, the daemon loads persisted state and attempts to clean up prior service processes and containers (best-effort) before restoring running services.

### Logging

The daemon captures `stdout` and `stderr` from managed services and makes logs available via IPC.

- **Buffering**: Recent logs are buffered in-memory.
- **Streaming**: The CLI can stream logs in real-time via the IPC channel.

## 4. Ephemeral Runtime, Persistent Context

A key design principle is that the **Runtime State** (PIDs, active connections) is ephemeral, but the **Contextual State** (History, Configuration) is persistent.

- If the daemon crashes, services are not adopted; on restart the daemon cleans up best-effort and restores configured/running services.
- On restart, the daemon reloads configuration and attempts to restore the desired state (restarting services), but it does not attempt to adopt pre-existing processes.
