# Architectural Decisions

## 001. Project Name: locald

**Context**: We need a name for the local development proxy/manager.
**Decision**: Use `locald`.
**Status**: Accepted.

## 002. Language: Rust

**Context**: High performance, safety, and single-binary distribution are desired.
**Decision**: Use Rust.
**Status**: Accepted.

## 003. Configuration: In-Repo

**Context**: Configuration should live with the code.
**Decision**: Use `locald.toml` in the project root.
**Status**: Accepted.

## 004. Architecture: Daemon + CLI

**Context**: Processes need to run in the background, independent of the terminal session.
**Decision**: Split into `locald-server` (daemon) and `locald-cli` (client).
**Status**: Accepted.

## 005. IPC: Unix Domain Sockets

**Context**: The CLI needs a low-latency, reliable way to send commands to the local daemon.
**Decision**: Use Unix Domain Sockets (specifically `/tmp/locald.sock`) with a newline-delimited JSON protocol.
**Status**: Accepted.

## 006. Port Assignment: Dynamic & Env Var

**Context**: Services need to know which port to listen on. Hardcoding ports leads to conflicts.
**Decision**: The daemon dynamically assigns a free port to each service and injects it as the `PORT` environment variable. Services must respect this variable.
**Status**: Accepted.

## 007. Daemonization: CLI-Managed

**Context**: Users expect `locald server` to run in the background without blocking the terminal.
**Decision**: The `locald-cli`'s `server` command spawns the `locald-server` binary as a detached child process. This keeps the server binary simple (foreground only) while providing a good UX.
**Status**: Accepted.

## 008. Daemon Detachment: setsid

**Context**: Simply spawning a background process isn't enough; if the CLI is killed (Ctrl-C), the child might die if it's in the same process group.
**Decision**: Use `setsid` when spawning `locald-server` to create a new session and fully detach it from the CLI's terminal.
**Status**: Accepted.

## 009. Server Idempotency

**Context**: Running `locald server` multiple times shouldn't cause errors or zombie processes.
**Decision**: The CLI checks if the daemon is already running (via IPC Ping) before attempting to start it. If running, it exits gracefully.
**Status**: Accepted.

## 010. Privileged Ports: Capabilities over Root

**Context**: We want to bind port 80 for clean URLs, but running the entire daemon as root violates Axiom 04 (Process Ownership).
**Decision**: The daemon runs as the user. We use `setcap cap_net_bind_service=+ep` on the binary to allow binding low ports. A `locald admin setup` command handles this.
**Status**: Accepted.

## 011. Hosts File: Section Management

**Context**: We need to map local domains to 127.0.0.1. Modifying `/etc/hosts` is risky and requires root.
**Decision**: We implement a safe "Section Manager" that only touches lines between `# BEGIN locald` and `# END locald`. The user runs `locald admin sync-hosts` with sudo to apply changes.
**Status**: Accepted.
