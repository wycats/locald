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
