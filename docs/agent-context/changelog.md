<!-- agent-template start -->
# Changelog

History of completed phases and key changes.
<!-- agent-template end -->

## Phase 2: Process Management (2025-11-30)
- Implemented `ProcessManager` in `locald-server` to spawn and manage child processes.
- Added `start`, `stop`, and `status` commands to `locald-cli` and IPC protocol.
- Implemented dynamic port assignment and `PORT` environment variable injection.
- Implemented `locald server` command to spawn the daemon in the background.
- Verified functionality with a dummy service.

## Phase 1: Scaffolding & Daemon Basics (2025-11-30)
- Initialized Rust workspace (`locald-server`, `locald-cli`, `locald-core`).
- Implemented `LocaldConfig` schema.
- Implemented `locald-server` with Tokio runtime.
- Implemented `locald-cli` with Clap.
- Implemented IPC via Unix Domain Sockets (`ping` command).
