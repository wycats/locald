# Phase 7 Walkthrough: Persistence & State Recovery

## Overview
In this phase, we are adding persistence to `locald`. This transforms it from a transient process runner into a proper daemon that maintains state across restarts.

## Key Decisions

### 1. JSON State File
We chose JSON for the state file because it's human-readable, easy to debug, and has excellent Rust support via `serde_json`. We store it in the XDG data directory (`~/.local/share/locald/`) to follow standard Linux conventions.

### 2. Process Recovery Strategy
When `locald` restarts, it reads the state file to see what was running.
*   **Decision**: Instead of trying to "adopt" running processes (which is difficult because we lose access to their stdout/stderr pipes), we chose to **kill and restart** them.
*   **Mechanism**: We check if the old PID exists using `nix::sys::signal::kill(pid, SIGTERM)`. Then we restart the service using the stored project path.
*   **Benefit**: This ensures a clean state and reconnects logs properly.

## Implementation Log

### Dependencies
- Added `serde` and `serde_json` to `locald-core` for state serialization.
- Added `directories` to `locald-server` for XDG path resolution.
- Added `nix` to `locald-server` for process signaling (killing zombies).

### Core Schema
- Defined `ServiceState` and `ServerState` in `locald-core/src/state.rs`.
- `ServiceState` captures: name, config, path, pid, port, status.

### State Manager
- Implemented `StateManager` in `locald-server/src/state.rs`.
- Handles loading/saving `state.json` asynchronously.

### Process Manager Integration
- Integrated `StateManager` into `ProcessManager`.
- `persist_state()` is called after `start()` and `stop()` operations.
- `restore()` is called on server startup. It:
    1. Loads the state file.
    2. Kills any lingering processes from the previous session.
    3. Restarts projects that had running services.

## Changes

### Codebase
- **`locald-core/src/state.rs`**: Added `ServiceState` and `ServerState` structs with Serde support.
- **`locald-server/src/state.rs`**: Implemented `StateManager` for loading/saving JSON state.
- **`locald-server/src/manager.rs`**: Added `persist_state` and `restore` logic.
- **`locald-server/src/proxy.rs`**: Refactored to use `axum` to resolve dependency conflicts and modernize the HTTP stack.
- **`locald-server/Cargo.toml`**: Added `directories`, `nix`, `axum`, `tower`, `tower-http`.

### Documentation
- Updated `task-list.md` to reflect completion.
- Updated `walkthrough.md` (this file).

## Verification
Verified manually:
1. Started `locald-server` and a dummy service (PID A).
2. Killed `locald-server`.
3. Restarted `locald-server`.
4. Verified dummy service was automatically restarted (PID B).
5. Confirmed state persistence works as expected.
