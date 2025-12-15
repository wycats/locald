# Phase 15 Walkthrough: Zero-Config SSL & Single Binary

## Overview

This phase focused on two major architectural improvements:

1.  **Single Binary**: Consolidating `locald-server` and `locald-cli` into a single executable (`locald`) to simplify distribution and updates.
2.  **Zero-Config SSL**: Implementing automatic, on-the-fly SSL certificate generation for `.localhost` domains, eliminating the need for manual certificate management.

## Changes

### 1. Single Binary Architecture

- **Refactoring**: The `locald-server` crate was converted into a library (`locald-server-lib`) that exposes a `run()` function.
- **CLI Integration**: The `locald-cli` crate now depends on `locald-server` and includes a `server` subcommand.
- **Commands**:
  - `locald server start`: Runs the server (daemon).
  - `locald server shutdown`: Sends a shutdown signal to the running daemon via IPC.
  - `locald up`: Now checks if the server is running and starts it in the background if needed.

### 2. Trust Store Management (`locald trust`)

- **Root CA**: We implemented a system to generate a self-signed Root CA (`locald Development CA`) if one doesn't exist.
- **Installation**: The `locald trust` command installs this Root CA into the system's trust store (requires `sudo`).
- **Library**: We used `ca_injector` (a fork/wrapper around `rust-openssl` or system commands) to handle the platform-specific installation.

### 3. On-the-Fly Certificate Generation

- **Library**: We used `rcgen` (v0.14) to generate certificates in memory.
- **CertManager**: A new `CertManager` component in the server:
  - Loads the Root CA key pair on startup.
  - Implements `rustls::server::ResolvesServerCert`.
  - Intercepts TLS handshakes (SNI).
  - Checks an in-memory cache for an existing certificate for the requested domain.
  - If missing, generates a new leaf certificate signed by the Root CA on the fly.
- **Fixes**:
  - **Rcgen 0.14 API**: We had to adapt to the new `CertifiedIssuer` API in `rcgen` 0.14, which requires a specific struct for signing rather than just a key pair.
  - **Rustls 0.23 Crypto Provider**: We encountered a runtime panic ("Could not automatically determine the process-level CryptoProvider"). We fixed this by explicitly installing the `ring` default provider in `locald-server/src/lib.rs`.

### 4. TLS Termination

- **Proxy**: The `ProxyManager` was updated to listen on port 443 (or 8443 if 443 is privileged/taken).
- **Rustls**: We integrated `rustls` to handle the TLS termination, using the `CertManager` to resolve certificates.
- **Domain**: We switched the default domain from `.local` to `.localhost` to better align with modern standards and browser behavior (which often force HTTPS for `.dev` and `.app`, and treat `.localhost` as secure context).

### 5. Seamless Updates (Dogfooding)

- **Versioning**: We added a `build.rs` script to `locald-cli` that injects a build timestamp into the version string (e.g., `0.1.0-1733182200`). This ensures that every `cargo install` results in a unique version, even if `Cargo.toml` hasn't changed.
- **Auto-Restart**: We updated `locald up` to check the running server's version via IPC. If the running version differs from the CLI's version, it automatically shuts down the old server and starts the new one. This allows for a seamless "install -> run" loop without manual shutdown steps.

## Verification

- **Trust**: `locald trust` successfully installed the CA.
- **HTTPS**: Verified using `curl --cacert ... https://shop-api.localhost:8443`. The connection was successful, and the certificate was verified as issued by our local CA.

### 6. Graceful Shutdown

- **Process Groups**: We refactored `ProcessManager` to spawn services in their own process groups (`setsid` / `setpgid`). This ensures that signals sent to the parent shell propagate to all child processes.
- **Protocol**: We implemented a robust shutdown protocol:
  1.  Send `SIGTERM` to the process group.
  2.  Wait 5 seconds for the process to exit.
  3.  If still running, send `SIGKILL` to the process group.
- **Verification**: Verified with a `dummy-service` script that traps signals.

### 7. Ad-Hoc Workflow Polish

- **Dynamic Ports**: `locald run` now finds a free port dynamically and injects it as `PORT`. It no longer saves this ephemeral port to `locald.toml` when generating a config.
- **Default Domain**: Updated `locald run` to generate configs with `.localhost` domains by default.
- **Argument Handling**: Fixed `locald run` (and `add`) to accept multiple arguments (e.g., `locald run pnpm dev`) without requiring quotes.
- **Ctrl+C Handling**: Updated `locald run` to catch `SIGINT` (Ctrl+C), allowing the child process to exit gracefully and the user to see the "Add to locald.toml?" prompt.

### 8. Polish & Bug Fixes

- **Global Config**: Implemented `GlobalConfig` to control `privileged_ports` and `fallback_ports`.
- **Strict Port Binding**: The server now strictly respects the configuration. If `privileged_ports` is true but binding fails (e.g., permission denied), it will error out unless `fallback_ports` is enabled.
- **Vite HMR Fix**: We diagnosed a "rapid redirect" loop with Vite caused by unhandled WebSocket upgrades. We implemented a fix in the proxy to explicitly reject `Upgrade` headers with `400 Bad Request`, forcing clients like Vite to fall back to HTTP polling until full WebSocket support is implemented (Phase 16).

## Verification
