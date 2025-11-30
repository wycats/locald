# Walkthrough - Phase 2: Process Management

**Goal**: The daemon can spawn and manage child processes based on configuration.

## Changes

### Process Manager
We implemented a `ProcessManager` in `locald-server` that maintains a registry of running services.
- It uses `tokio::process::Command` to spawn child processes.
- It automatically assigns a free port and injects it as `PORT` environment variable.
- It captures `stdout` and `stderr` (currently inheriting to the daemon's output).

### IPC Protocol
We added new commands to the IPC protocol:
- `Start { path }`: Tells the daemon to read `locald.toml` at the given path and start the services defined therein.
- `Stop { name }`: Stops a service by name.
- `Status`: Returns a list of services with their PID, port, and status.

### CLI
We added corresponding subcommands to the CLI:
- `locald server`: Starts the `locald-server` daemon in the background (detached).
- `locald start [path]`: Starts the project in the given path (defaults to current dir).
- `locald stop <name>`: Stops a service.
- `locald status`: Lists running services.

## Verification Results
We verified the functionality using a dummy service script (`examples/dummy-service`).
- The daemon starts in the background.
- The service starts and receives a random port.
- `locald status` correctly reports the service as running.
- `locald stop` terminates the process.
