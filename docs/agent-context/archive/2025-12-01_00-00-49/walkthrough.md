# Phase 12 Walkthrough: Docker Integration

## Overview

In this phase, we added support for running Docker containers as services. This allows users to define dependencies like databases or caches directly in `locald.toml`.

## Key Decisions

- **Library Selection**: We chose `bollard` for direct Docker API access over wrapping the `docker` CLI. This provides better control over the lifecycle and log streaming without spawning intermediate shell processes.
- **Networking**: We map a dynamic host port to the container's exposed port to maintain our "managed ports" philosophy. The `container_port` must be specified in the config.
- **Unified Manager**: We refactored `ProcessManager` to handle both `Child` processes and Docker `container_id`s in a single `Service` struct, simplifying the architecture.

## Changes

### `locald-core`

- Updated `ServiceConfig` to include `image` (Option<String>) and `container_port` (Option<u16>).
- Updated `ServiceState` to include `container_id` for persistence.

### `locald-server`

- Added `bollard` and `futures-util` dependencies.
- Refactored `manager.rs`:
  - `Service` struct now holds `container_id`.
  - `start` method branches based on presence of `image` field.
  - Added `start_container` method to handle Docker lifecycle (create, start, log stream).
  - Updated `stop`, `list`, `shutdown`, `persist_state`, and `restore` to handle containers.
- Implemented log streaming from Docker to the existing broadcast channel.

### `locald-cli`

- Updated `init` command to include commented-out Docker fields in the generated config.

## Verification

- Created `examples/docker-service` with a Redis container and a dependent worker process.
- Verified that `locald` can start the Redis container, assign a port, and stream logs.
- Verified that the worker process starts after Redis.
- Verified that `locald status` shows both services running.
- Verified that `locald stop` cleans up the container.

## Refactor: Self-Daemonization

After the initial Docker implementation, we refactored `locald-server` to handle its own daemonization, removing the need for shell backgrounding (`&`).

- **`daemonize` Crate**: Used to fork the process, detach from the terminal, and manage PID files.
- **Idempotency**: The server now checks if the IPC socket is already active before starting. If it is, it exits gracefully, preventing duplicate daemons.
- **`--foreground` Flag**: Added a CLI flag to skip daemonization for debugging purposes.
- **Logging**: Daemon logs are now redirected to `/tmp/locald.out` and `/tmp/locald.err`.
