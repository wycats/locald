# Phase 13 Implementation Plan: Smart Health Checks

## Goal

Ensure services are fully ready to accept connections before dependent services are started. Minimize configuration burden by using "Zero-Config" detection strategies.

## User Requirements

- **App Builder**: "I want my API to wait until Redis is actually ready, not just started."
- **Power User**: "I want to see _why_ a service is considered healthy (Docker, Port, etc.) in the status output."
- **Zero-Config**: Standard setups (Postgres in Docker, Node app on port) should work without extra config.

## Strategy: The "Zero-Config" Hierarchy

1.  **Docker Native**: If `image` is present AND container has `HEALTHCHECK`, use it.
2.  **Notify Socket**: Always set `NOTIFY_SOCKET`. If the app sends `READY=1`, mark healthy.
3.  **TCP Probe**: If `port` is defined, wait for it to accept connections.
4.  **Explicit Config**: User defines `health_check` in `locald.toml`.

## Architecture Changes

### `locald-core`

- Update `ServiceConfig` to add optional `health_check` field (for explicit overrides).
- Update `ServiceState` (or runtime struct) to track:
  - `health_status`: `Unknown`, `Starting`, `Healthy`, `Unhealthy`.
  - `health_source`: `Docker`, `Notify`, `Tcp`, `Explicit`, `None`.

### `locald-server`

- **Notify Listener**: Create a `NotifyServer` that manages a Unix Datagram socket and maps incoming packets to service IDs.
- **Docker Monitor**: Poll `inspect_container` or listen to events for health status changes.
- **TCP Prober**: A background task that attempts to connect to `localhost:port` if no other health check is active.
- **Dependency Logic**: Update the startup sequence. `depends_on` currently waits for `Process` to exist. It must now wait for `health_status == Healthy`.

### `locald-cli`

- Update `status` command to display health status and source.
- (Optional) Warn if a service has `health_source: None`.

## Step-by-Step Plan

### Step 1: Core Data Structures

- [ ] Update `locald-core` structs.
- [ ] Add `health_check` to `locald.toml` parser.

### Step 2: Notify Socket (The Standard)

- [ ] Implement `NotifyServer` in `locald-server`.
- [ ] Inject `NOTIFY_SOCKET` env var into child processes.
- [ ] Handle `READY=1` messages.

### Step 3: Docker Integration

- [ ] Update `start_container` to check for `HEALTHCHECK` config.
- [ ] Implement polling/event loop for Docker health.

### Step 4: TCP Fallback

- [ ] Implement TCP connect loop for services with `port` but no other health check.

### Step 5: Dependency Gating

- [ ] Update `ServiceManager::start` to block (async) or defer dependent startup until health check passes.

### Step 6: UI/CLI Updates

- [ ] Update `locald status` output.
