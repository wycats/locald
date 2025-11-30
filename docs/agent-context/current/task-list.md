# Task List - Phase 2: Process Management

- [x] **IPC Protocol**
  - [x] Update `locald-core/src/ipc.rs` with `Start`, `Stop`, `Status` commands.
  - [x] Update `locald-core/src/ipc.rs` with `ServiceStatus` response structures.

- [x] **Process Manager Core**
  - [x] Create `locald-server/src/process.rs` (or `manager.rs`).
  - [x] Implement `Service` struct to hold child process handle and config.
  - [x] Implement `ProcessManager` struct with `start`, `stop`, `list` methods.
  - [x] Implement port assignment logic (find free port).
  - [x] Integrate `ProcessManager` into `locald-server/src/main.rs` (shared state).

- [x] **CLI Implementation**
  - [x] Update `locald-cli/src/main.rs` with new subcommands.
  - [x] Implement `start` command handler (read `locald.toml`, send absolute path to daemon).
  - [x] Implement `stop` command handler.
  - [x] Implement `status` command handler.

- [x] **Verification**
  - [x] Verify `start` spawns process.
  - [x] Verify `PORT` is injected.
  - [x] Verify `stop` kills process.
