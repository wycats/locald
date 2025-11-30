# Implementation Plan - Phase 2: Process Management (The Supervisor)

**Goal**: The daemon can spawn and manage child processes based on configuration.

## Proposed Changes

### 1. IPC Protocol Updates (`locald-core`)
- Update `ipc::Command` to support process management:
  - `Start { path: PathBuf }`: Register and start a service from a directory (reads `locald.toml`).
  - `Stop { name: String }`: Stop a running service.
  - `Status`: List running services.
- Update `ipc::Response` to return service status info.

### 2. Process Manager (`locald-server`)
- Create a `ProcessManager` struct to manage child processes.
- State management: `HashMap<String, ServiceState>`.
- Functionality:
  - `spawn(config: LocaldConfig)`: Start a process using `tokio::process::Command`.
  - `kill(name: String)`: Terminate a process.
  - `get_status()`: Return list of running services.
- **Environment Variables**:
  - Automatically assign a free `PORT`.
  - Inject `PORT` into the child process environment.
- **Output Capture**:
  - Capture `stdout` and `stderr`.
  - For this phase, log output to the daemon's stdout/tracing logs (preparation for Phase 4 logs).

### 3. CLI Updates (`locald-cli`)
- Add subcommands:
  - `locald start .`: Start the project in the current directory.
  - `locald stop <name>`: Stop a service.
  - `locald status`: Show running services.

## User Verification
- [x] Run `locald server` in one terminal.
- [x] Create a dummy server script (e.g., a simple python http server or rust hello world) with a `locald.toml`.
- [x] Run `locald start .` in the dummy project.
- [x] Verify the process starts and is assigned a port.
- [x] Verify `locald status` shows the service.
- [x] Verify `locald stop <name>` stops the process.
