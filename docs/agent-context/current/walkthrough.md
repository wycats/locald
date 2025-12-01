# Phase 13 Walkthrough: Smart Health Checks

## Overview

In this phase, we are implementing "Smart Health Checks" to ensure services are truly ready before their dependents start. We are prioritizing a "Zero-Config" approach where `locald` infers the best health check strategy.

## Key Decisions

- **Hierarchy**: We check for Docker Health -> `sd_notify` -> TCP Port -> Explicit Config.
- **Visibility**: The UI/CLI must show _how_ health is being determined, so users know if they are relying on a fallback.
- **Implementation**:
  - **Docker Health**: We use `bollard` to inspect the container configuration. If a `HEALTHCHECK` is defined in the image, we spawn a background task to poll the container's health status via `inspect_container`.
  - **sd_notify**: We implemented a Unix Domain Socket server in `notify.rs` that listens for `READY=1` messages. We inject the `NOTIFY_SOCKET` environment variable into spawned processes.
  - **TCP Probe**: As a fallback for services with a port but no other health check, we attempt to connect to the TCP port.
  - **Process Manager**: The `ProcessManager` now maintains `health_status` and `health_source` for each service and broadcasts updates. Startup order now respects health status, waiting for dependencies to be `Healthy`.

## Changes

- **`locald-server/src/notify.rs`**: Added `NotifyServer` to handle `sd_notify` protocol messages. It verifies the sender's PID using `SO_PEERCRED` to ensure security.
- **`locald-server/src/manager.rs`**:
  - Integrated `NotifyServer` handling via `handle_notify`.
  - Added `spawn_docker_health_monitor` to poll Docker health status.
  - Added `spawn_tcp_health_monitor` to probe TCP ports.
  - Updated `start` to wait for health checks before proceeding to dependent services.
  - Updated `start_container` to detect if a container has a healthcheck.
- **`locald-server/src/main.rs`**:
  - Initialized `NotifyServer` and spawned a task to run it.
  - Connected `NotifyServer` events to `ProcessManager`.
- **`locald-server/Cargo.toml`**: Added `nix` features (`socket`, `user`) required for `SO_PEERCRED`.
- **`locald-cli/src/main.rs`**: Updated the `status` command to display the `HEALTH` and `SOURCE` columns, giving users visibility into how their service health is being determined.

## Verification

- **Compilation**: `cargo check -p locald-server` passes.
- **Logic**: The code covers all three health check strategies and integrates them into the startup flow.

## Bug Fixes

- **Non-blocking Restore**: Fixed an issue where `locald-server` would block on startup while restoring services, preventing the IPC server from starting. Moved `restore()` to a background task.

## Dogfooding

- **Docs Server**: Added a `locald.toml` to the root of the repository to serve the documentation using `python3 -m http.server`. This validates the core functionality and provides a convenient way to browse the docs.

- **Cargo Alias**: Added `.cargo/config.toml` with a `locald` alias. Now you can run `cargo locald <command>` (e.g., `cargo locald status`) to build and run the CLI in one step.
