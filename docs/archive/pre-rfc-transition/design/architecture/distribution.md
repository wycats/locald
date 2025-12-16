# Architecture: Distribution & Lifecycle

**Goal**: `locald` is distributed as a single, self-contained binary that manages its own lifecycle, updates, and privileged operations without external dependencies.

## 1. The Single Binary Model

Originally, `locald` consisted of `locald-server` (the daemon) and `locald-cli` (the client). In Phase 15, these were merged into a single `locald` binary.

### Why?

- **Version Alignment**: Eliminates "Client version X vs Server version Y" compatibility issues.
- **Deployment**: Users only need to download one file.
- **Workflow**: The "Try -> Save -> Run" workflow is smoother when the CLI can spawn the server directly from the same executable.

### Implementation

The `locald` binary uses `clap` subcommands to distinguish modes:

- `locald server`: Runs the daemon (long-running process).
- `locald status`, `locald up`, etc.: Runs as a client, communicating with the daemon via Unix Domain Socket.

## 2. Embedded Assets

`locald` is not just a binary; it's a platform that includes a Dashboard (Svelte) and Documentation (Astro).

### Mechanism

We use [`rust-embed`](https://crates.io/crates/rust-embed) to compile these static assets directly into the binary.

- **Build Time**: `build.rs` in `locald-server` copies the `dist/` folders from `locald-dashboard` and `locald-docs`.
- **Runtime**: The internal proxy serves these assets from memory when requests match specific hostnames (`locald.localhost`, `docs.localhost`).

This ensures that the Dashboard and Docs are always perfectly synced with the binary version.

## 3. Self-Upgrading Lifecycle

`locald` manages its own updates to ensure users are always on the latest version without breaking their running services.

### The `locald up` Protocol

When a user runs `locald up` (or `locald selfupgrade` in the future):

1.  **Download**: The new binary is downloaded to a temporary location.
2.  **Replace**: The current binary on disk is replaced atomically.
3.  **Signal**: The client sends a `SIGTERM` to the running daemon.
4.  **Graceful Shutdown**: The daemon stops accepting new requests, waits for active connections to drain (with a timeout), and saves its state (running services, PIDs).
5.  **Restart**: The client (or a supervisor) starts the new daemon.
6.  **Restore**: The new daemon loads the state file and "adopts" the running processes. It does _not_ kill them. It reconnects to their stdout/stderr pipes if possible (or just monitors the PID).

## 4. Privilege Separation (`locald-shim`)

To provide a "Production-Like" environment, `locald` needs to bind to privileged ports (80 and 443). However, running the entire daemon as `root` is insecure and messes up file permissions for logs and build artifacts.

### The Solution: `locald-shim`

We use a small, focused binary called `locald-shim` that has the `setuid` bit set (or `CAP_NET_BIND_SERVICE` capability).

- **Role**: It does _one_ thing: binds a socket and passes the file descriptor to the unprivileged `locald` process.
- **Workflow**:
  1. `locald` starts up as a normal user.
  2. It detects it needs port 80/443.
  3. It executes `locald-shim`.
  4. `locald-shim` binds the port and sends the file descriptor back to `locald` over a Unix socket.
  5. `locald-shim` exits.
  6. `locald` uses the file descriptor to accept connections.

This ensures that the complex logic of the daemon (parsing config, running user code) always happens as the unprivileged user.
