# Phase 1 Walkthrough

## Changes

### Workspace Initialization
Initialized a Rust workspace with three crates:
- `locald-server`: The background daemon process.
- `locald-cli`: The command-line interface tool.
- `locald-core`: Shared library for types and configuration.

### Configuration Schema
Defined the `LocaldConfig` struct in `locald-core`. It supports:
- Project metadata (name, domain).
- Multiple services per project.
- Managed ports (optional `port` field).
- Environment variables and working directory overrides.

### Server Entrypoint
Implemented the `locald-server` entrypoint in `locald-server/src/main.rs`.
- Uses `tokio` for the async runtime.
- Uses `tracing` for structured logging.
- Handles graceful shutdown on Ctrl+C.

### CLI Entrypoint
Implemented the `locald-cli` entrypoint in `locald-cli/src/main.rs`.
- Uses `clap` for argument parsing.
- Defines `ping` and `server` subcommands.

### IPC Implementation
Implemented Inter-Process Communication using Unix Domain Sockets.
- **Protocol**: JSON over Unix Socket (defined in `locald-core`).
- **Server**: Listens on `/tmp/locald.sock` and handles `Ping` requests.
- **Client**: Connects to the socket and sends requests.
- **Verification**: `locald ping` sends a `Ping` request and expects a `Pong` response.
