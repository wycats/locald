# Phase 1 Walkthrough: Scaffolding & Basic IPC

## Overview
In this phase, we established the foundational structure of the `locald` project. We set up a Rust workspace with three crates: `locald-server` (the daemon), `locald-cli` (the user interface), and `locald-core` (shared types and configuration). We also implemented a basic IPC mechanism using Unix Domain Sockets to allow the CLI to communicate with the server.

## Changes

### 1. Workspace Structure
We created a Cargo workspace with the following members:
- **`locald-core`**: Contains shared definitions, including the `LocaldConfig` struct (using `serde`) and the IPC protocol (`IpcRequest`, `IpcResponse`).
- **`locald-server`**: The long-running daemon process. It currently sets up a Tokio runtime, handles `SIGINT` for graceful shutdown, and runs the IPC server.
- **`locald-cli`**: The command-line interface. It parses arguments using `clap` and communicates with the server via the IPC socket.

### 2. IPC Mechanism
We implemented a simple request/response protocol over Unix Domain Sockets (`/tmp/locald.sock`).
- The server listens on the socket and spawns a task for each incoming connection.
- The protocol uses newline-delimited JSON for simplicity and debuggability.
- We implemented a `Ping` command to verify connectivity.

### 3. Configuration Schema
We defined the initial `LocaldConfig` schema in `locald-core`. This schema is designed to be decentralized, living in `locald.toml` files within project repositories.

### 4. Design Documentation
We fleshed out the architectural principles of the project:
- **Design Axioms**: A set of 6 core principles guiding the development (Decentralized Config, Daemon First, Managed Ports/DNS, Process Ownership, Interface Parity, 12-Factor).
- **Interaction Modes**: Defined the different ways users will interact with the system (Foreman, Hostctl, etc.).

## Verification
We verified the implementation by running the `locald-server` and executing `locald-cli ping`. The server successfully received the request and responded with "Pong".

## Next Steps
With the foundation in place, we are ready to move to **Phase 2: Process Management**, where we will implement the core logic for spawning and managing child processes defined in the configuration.
