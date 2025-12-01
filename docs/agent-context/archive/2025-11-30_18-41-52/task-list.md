# Phase 7 Task List

- [x] **Core: State Schema**
  - [x] Create `locald-core/src/state.rs`.
  - [x] Define `ServiceState` and `ServerState` structs.
  - [x] Add `serde` derives.

- [x] **Server: State Manager**
  - [x] Create `locald-server/src/state.rs`.
  - [x] Implement `load()` and `save()`.
  - [x] Handle XDG paths (`directories` crate).

- [x] **Server: Integration**
  - [x] Add `StateManager` to `ProcessManager`.
  - [x] Call `save()` on service start/stop.
  - [x] Implement `restore()` method in `ProcessManager`.

- [x] **Server: Reconciliation**
  - [x] Implement PID liveness check (`nix` crate).
  - [x] Handle port re-binding for restored services (handled by `start` logic).

- [x] **Verification**
  - [x] Verify persistence across restarts.
  - [x] Verify zombie cleanup/adoption.
