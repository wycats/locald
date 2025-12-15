# Phase 15 Implementation Plan: Zero-Config SSL, Single Binary, and CLI Polish

**Goal**: Enable HTTPS support for `.localhost` domains using a pure Rust stack, simplify installation by merging binaries, and improve the "Try -> Save -> Run" workflow.

## 1. Single Binary Architecture

Refactor the codebase to produce a single `locald` binary that acts as both client and server.

- [x] **Refactor Server**: Move `locald-server` logic into a library crate `locald-server-lib`.
- [x] **Unified CLI**: Update `locald-cli` to depend on `locald-server-lib`.
- [x] **Subcommands**:
  - `locald server start`: Runs the daemon process (foreground).
  - `locald server shutdown`: Sends a shutdown signal to the running daemon.
  - `locald up`: Starts the daemon (background) if needed, then registers the current project.
  - `locald status`: Shows running services (formerly `locald list`?).

## 2. "Try -> Save -> Run" Workflow

Implement a smooth workflow for ad-hoc execution and configuration generation.

- [x] **`locald run <cmd>`**:
  - Runs `<cmd>` as a child process of the CLI (attached).
  - Sets up environment variables (PORT, etc.) similar to the daemon.
  - On exit (or Ctrl-C), prompt the user: _"Do you want to add this command to locald.toml? [y/N]"_.
  - If yes, prompt for a service name and append to `locald.toml` (creating it if missing).
- [x] **`locald add <name> <cmd>`**:
  - Explicitly adds a service to `locald.toml`.

## 3. Zero-Config SSL

Enable HTTPS by default for `.localhost` domains.

- [x] **Trust Store Injection**:
  - [x] Integrate `rcgen` (generation) and `ca_injector` (installation).
  - [x] Implement `locald trust` command to generate a Root CA and install it into the system trust store.
- [ ] **On-the-fly Signing**:
  - [x] Implement a `CertManager` in the daemon (In Progress: Compilation errors).
  - [x] Use `rcgen` to generate leaf certificates signed by the Root CA on demand.
  - [x] Implement `rustls::ResolvesServerCert` to serve these certificates dynamically.
- [ ] **Proxy Update**:
  - [x] Update the internal proxy to handle TLS termination (In Progress: Integration started).
  - [x] Switch default TLD from `.local` to `.localhost`.

## 4. Verification

- [x] Verify `locald run` workflow.
- [ ] Verify `locald shutdown` cleanly stops the daemon.
- [ ] Verify `.localhost` domains work in Chrome/Firefox without warnings.
