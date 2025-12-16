# Research: Smart Health Checks

## Goal

Minimize user configuration for service readiness. Users should not have to write manual health checks for standard use cases.

## Findings

### 1. Docker Native Health

The `bollard` crate exposes `State.Health.Status` from the Docker API.

- **Strategy**: If a container has a `HEALTHCHECK` instruction, `locald` should automatically respect it.
- **Implementation**: Poll `inspect_container` or listen to events.

### 2. Systemd `sd_notify`

This is the standard for Linux daemons to signal readiness.

- **Mechanism**: Manager sets `NOTIFY_SOCKET` env var. Child sends `READY=1` to that socket.
- **Pros**: Supported by many standard Linux tools (Postgres, Nginx, etc.) and easy for users to add to their own scripts.
- **Implementation**: `locald` creates a Unix Datagram socket for each service and listens for the signal.

### 3. TCP Port Probing

Since `locald` manages ports, we know where the service _should_ be listening.

- **Strategy**: If a `port` is defined, default to a TCP connect loop.
- **Pros**: Works for almost any network service.
- **Cons**: "Port Open" != "App Ready" (sometimes), but it's a very good 90% solution.

## Proposed "Zero-Config" Hierarchy

1.  **Docker Health**: If `image` is present AND container has `HEALTHCHECK`, use it.
2.  **Notify Socket**: Always set `NOTIFY_SOCKET`. If the app uses it, great.
3.  **TCP Probe**: If `port` is defined, wait for it to accept connections.
4.  **Explicit Config**: User defines `health_check = "..."` in `locald.toml`.

## Phase 13 Plan Refinement

We should implement this hierarchy. It makes `locald` feel "magical" and robust.
