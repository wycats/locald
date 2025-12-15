# Implementation Plan - Phase 23 (Part 2): Foreman Parity

## 1. .env Support

We need to load `.env` files from the project root and merge them into the service's environment variables.

### Changes
- **`locald-server/src/manager.rs`**:
    - In `start()` or `apply_config()`, check for `.env` file in `path`.
    - Parse `.env` file (using `dotenvy` or manual parsing).
    - Merge into `resolved_env`.
    - Precedence: `locald.toml` > `.env` > System (inherited).

### Dependencies
- Add `dotenvy` to `locald-server/Cargo.toml`.

## 2. Signal Handling

Some applications (like Nginx or some worker queues) require specific signals to shut down gracefully (e.g., `SIGQUIT` vs `SIGTERM`).

### Changes
- **`locald-core/src/config.rs`**:
    - Add `stop_signal: Option<String>` to `CommonServiceConfig`.
    - Default to `"SIGTERM"`.
- **`locald-server/src/manager.rs`**:
    - Update `terminate_process` to accept the signal type.
    - Parse the string to `nix::sys::signal::Signal`.

## 3. Verification

- Create a test case with a `.env` file and verify the env var is present in the service.
- Create a test case with a service that traps `SIGINT` and verify it shuts down correctly when configured.
