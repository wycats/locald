# Phase 7 Implementation Plan: Persistence & State Recovery

## Goal
Ensure that `locald` can survive a restart without losing track of registered services. Currently, the daemon is stateless; if it stops, it forgets everything. We need to persist the state to disk and restore it on startup.

## User Requirements
- **Persistence**: When I register/start a service, `locald` should remember it.
- **Recovery**: If I restart `locald`, it should try to restore the previous state.
- **Resilience**: If a service process died while `locald` was down, `locald` should detect that and mark it as stopped (or restart it if configured to).
- **Cleanup**: If `locald` crashes, it shouldn't leave zombie processes that block ports for the next run.

## Architecture

### 1. State File
- **Location**: `$XDG_DATA_HOME/locald/state.json` (e.g., `~/.local/share/locald/state.json`).
- **Format**: JSON.
- **Content**: List of registered services (path, config, last known PID).

### 2. State Manager
- A new module in `locald-server` responsible for:
    - Loading state on startup.
    - Saving state whenever the service list changes (start/stop).
    - Periodically saving state (optional, but good for safety).

### 3. Process Reconciliation (The "Zombie Hunter")
- On startup, `locald` will read the state file.
- For each service:
    - Check if the PID still exists.
    - Check if the PID actually belongs to the service (tricky, maybe check command line or just assume if it's alive).
    - If alive: Adopt it.
    - If dead: Mark as stopped.

## Step-by-Step Plan

### Step 1: Define State Schema
- [ ] Create `locald-core/src/state.rs` with `ServerState` struct.
- [ ] Add serialization/deserialization (Serde).

### Step 2: Implement State Persistence
- [ ] Implement `StateManager` in `locald-server`.
- [ ] Integrate `StateManager` into `ProcessManager`.
- [ ] Save state on `start()` and `stop()`.

### Step 3: Implement State Restoration
- [ ] Load state in `main.rs` before creating `ProcessManager`.
- [ ] Populate `ProcessManager` with restored services.

### Step 4: Process Reconciliation
- [ ] Implement logic to check if a PID is alive and valid.
- [ ] Handle "adoption" of existing processes.

### Step 5: Verification
- [ ] Test: Start service -> Kill daemon -> Start daemon -> Verify service is still listed.
- [ ] Test: Start service -> Kill daemon -> Kill service -> Start daemon -> Verify service is marked stopped.
