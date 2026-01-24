---
title: Shared Utility Crate
stage: 3
---

# RFC 0067: Shared Utility Crate

## Summary

Create a shared utility crate (e.g., `locald-utils` or `locald-common`) to house general-purpose functionality that is currently duplicated or buried within specific components. This will improve code reuse, testability, and security.

## Motivation

As `locald` grows, we are finding pieces of logic that are not specific to the daemon, CLI, or builder, but are rather general-purpose systems programming tasks.

Examples found so far:

1.  **OCI Tar Extraction**: The logic to safely extract OCI image layers (handling hard links and whiteout files) is complex and currently lives in `locald-builder`.
2.  **Path Sanitization**: Ensuring paths don't escape a root directory is needed in multiple places (builder, server static files, etc.).
3.  **OCI Layout Operations**: Parsing and manipulating OCI layouts.

## Proposal

1.  Create a new crate in the workspace.
2.  Migrate identified utilities into this crate.
3.  Add rigorous unit tests for these utilities, as they are often security-critical (e.g., path traversal prevention).

## Initial Candidates

### 1. Filesystem Utilities (`fs`)

**Candidates**: `safe_join`, `unpack_hard_link`, `ensure_gitignore`

- **Description**: Security-critical path manipulation and file operations.
- **Analysis**:
  - `safe_join`: While crates like `path-clean` exist, securely joining paths to prevent traversal (especially in the presence of symlinks) is a subtle security boundary often specific to the application's threat model. We need strict guarantees that a path never escapes a root, which is "novel" enough to warrant a dedicated, audited wrapper.
  - `unpack_hard_link`: Specific to OCI layer extraction quirks.
- **Recommendation**: Extract to `locald-utils::fs`.

### 2. Process Management (`process`)

**Candidates**: `terminate_gracefully`, `kill_pid`

- **Description**: Robust process termination (SIGTERM -> wait -> SIGKILL) and signal wrappers.
- **Analysis**:
  - The "graceful shutdown" pattern is common, but correct implementation with `tokio` timeouts and proper signal handling (via `nix`) is often verbose and error-prone. Existing crates like `process-control` exist but may bring unnecessary dependencies or mismatching async runtimes.
  - **Novelty**: High-reliability process supervision integrated with our specific logging and async runtime.
- **Recommendation**: Extract to `locald-utils::process`.

### 3. Network Probing (`probe`)

**Candidates**: `wait_for_tcp`, `wait_for_http`

- **Description**: Health checks for services.
- **Analysis**:
  - This is a standard problem. Crates like `wait-for-it` exist as CLI tools, but as a library, it's usually a simple loop around `TcpStream::connect`.
  - **Recommendation**: Extract to `locald-utils::probe`. Consider using `tokio-retry` internally to simplify the loop logic, but keep the API simple and focused on our use cases (e.g., specific timeout defaults).

### 4. Certificate Authority (`cert`)

**Candidates**: `CaManager` (unifying CLI and Server logic)

- **Description**: Management of a local development CA, generating leaf certificates, and installing the root CA to the system trust store.
- **Analysis**:
  - We currently use `rcgen` for the crypto. The "novelty" here is the **lifecycle management**: creating the CA if missing, saving it securely, and interacting with the OS trust store (which varies by OS).
  - **Recommendation**: Extract to `locald-utils::cert`. This unifies the split logic where the CLI installs the CA and the Server uses it.

### 5. IPC Configuration (`ipc`)

**Candidates**: `socket_path`

- **Description**: Determines the Unix domain socket path based on environment variables (`LOCALD_SOCKET`, `LOCALD_SANDBOX_ACTIVE`).
- **Analysis**:
  - **Novelty**: This is purely domain logic specific to `locald`'s configuration and sandbox model. No external crate can solve this.
  - **Recommendation**: Extract to `locald-utils::ipc` to resolve the circular dependency where `locald-cli` depends on `locald-server` just for this path.

### 6. Systemd Notification Server (`notify`)

**Candidates**: `NotifyServer`

- **Description**: A server that listens for `sd_notify` messages (e.g., `READY=1`) from child processes.
- **Analysis**:
  - Most `sd_notify` crates (like `sd-notify`) are designed for the **client** (the service sending the notification). `locald` acts as the **supervisor**, so it needs to _receive_ these notifications. There are fewer off-the-shelf solutions for the server side of the protocol in Rust.
  - **Recommendation**: Extract to `locald-utils::notify`. This allows us to easily test the notification logic in isolation or use it in test harnesses.

### 7. Environment & Configuration (`env`)

**Candidates**: `get_home_dir`, `get_xdg_data_home`, `is_sandbox_active`, `get_shim_path`

- **Description**: Centralized access to standard (`HOME`, `XDG_*`) and application-specific (`LOCALD_*`) environment variables.
- **Analysis**:
  - **Duplication**: `std::env::var("HOME")` with error handling is repeated in `locald-cli`, `locald-builder`, and `locald-server`.
  - **Consistency**: Logic for `LOCALD_SANDBOX_ACTIVE` and `LOCALD_SHIM_PATH` is scattered, risking drift between components.
  - **Novelty**:
    - For standard paths, the **`dirs`** crate is the standard solution. We should use it instead of manual `env::var("HOME")` lookups.
    - For `LOCALD_*` vars, a custom wrapper is needed.
  - **Recommendation**: Extract to `locald-utils::env`. Use `dirs` internally for standard paths.

## Summary & Prioritization

1.  **`ipc` & `env`**: **Immediate**. Fixes circular dependencies and configuration drift.
2.  **`fs`**: **High**. Security critical.
3.  **`cert`**: **High**. Unifies split logic.
4.  **`process`**: **Medium**. Robustness.
5.  **`probe`**: **Medium**. Cleanup (adopt `tokio-retry`).
6.  **`notify`**: **Low**. Cleanup.
