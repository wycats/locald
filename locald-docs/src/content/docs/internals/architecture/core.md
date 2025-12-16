---
title: "Architecture: Core"
---

This document describes the core architecture of `locald`, including the daemon lifecycle, IPC, and process supervision.

## 1. Daemon + CLI Split

The system is composed of two logical components (which may be distributed as a single binary):

- **Daemon (`locald-server`)**: A long-running background process that manages the state of services, logs, and configuration. It is responsible for the "truth" of the system.
- **CLI (`locald-cli`)**: An ephemeral command-line tool that sends commands to the daemon and displays the results.

### Daemonization

The daemon manages its own backgrounding (self-daemonization) rather than relying on the shell or an external supervisor. This ensures consistent behavior across environments. It handles:

- Detaching from the terminal (setsid).
- Managing its own PID file.
- Redirecting its own stdout/stderr to logs.

## 2. Inter-Process Communication (IPC)

Communication between the CLI and the Daemon happens over **Unix Domain Sockets**.

- **Socket Path**: Typically `/tmp/locald.sock` (or `$XDG_RUNTIME_DIR/locald.sock`).
- **Protocol**: Newline-delimited JSON.
- **Security**: File permissions on the socket restrict access to the user who started the daemon.

## 3. Process Supervision

The daemon acts as a supervisor for child processes (services).

### Lifecycle

- **Spawn**: Services are spawned as child processes.
- **Environment**: The daemon injects environment variables (e.g., `PORT`, `DATABASE_URL`) into the child process.
- **Monitoring**: The daemon monitors the exit status of child processes.
- **Recovery**:
  - **Restart**: If a service crashes, the daemon can restart it (configurable).
  - **Zombie Cleanup**: On startup, the daemon checks for "zombie" processes from a previous session (using the PID from the persisted state) and kills them before starting fresh. This prevents orphaned processes from holding ports.

### Logging

The daemon captures the `stdout` and `stderr` of all child processes.

- **Persistence**: Logs are written to disk (e.g., `~/.local/share/locald/logs/<service>.log`) so they survive daemon restarts.
- **Streaming**: The CLI can stream these logs in real-time via the IPC channel.

## 4. Ephemeral Runtime, Persistent Context

A key design principle is that the **Runtime State** (PIDs, active connections) is ephemeral, but the **Contextual State** (Logs, History, Configuration) is persistent.

- If the daemon crashes, the logs remain available for debugging.
- On restart, the daemon reloads the configuration and attempts to restore the desired state (restarting services), but it does not try to "adopt" running processes (see Zombie Cleanup).
