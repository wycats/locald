<!-- agent-template start -->
# Changelog

History of completed phases and key changes.
<!-- agent-template end -->

## Phase 1: Scaffolding & Daemon Basics (2025-11-30)
- Initialized Rust workspace (`locald-server`, `locald-cli`, `locald-core`).
- Implemented `LocaldConfig` schema.
- Implemented `locald-server` with Tokio runtime.
- Implemented `locald-cli` with Clap.
- Implemented IPC via Unix Domain Sockets (`ping` command).
