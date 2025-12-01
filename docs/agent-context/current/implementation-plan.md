# Phase 11 Implementation Plan: Docker Integration

## Goal
Support "Hybrid" development where some services run locally (binaries) and others run in Docker containers (databases, queues), all managed by `locald`.

## User Requirements
- **App Builder**: "I want to add a Postgres database to my project without writing a `docker-compose.yml`."
- **Power User**: "I want `locald` to manage the ports for my containers so they don't conflict."

## Strategy
1.  **Schema Update**: Add `image`, `container_port`, and `volumes` to `ServiceConfig`.
2.  **Process Manager**:
    - Detect if a service is a Docker service (`image` is present).
    - Construct `docker run` commands instead of shell commands.
    - Ensure robust cleanup (remove old containers on start).
3.  **Port Management**:
    - Bind ephemeral host ports and map them to `container_port`.
    - Inject these ports into other services.

## Step-by-Step Plan

### Step 1: Schema Update
- [ ] Update `locald-core/src/config.rs` to add `image`, `container_port`, `volumes`.
- [ ] Update `locald-cli/src/init.rs` (optional, maybe just leave as manual edit for now).

### Step 2: Docker Process Logic
- [ ] In `locald-server/src/manager.rs`, modify `start()`:
    - Check if `service.image` is set.
    - If so, run `docker rm -f locald-<project>-<service>` first.
    - Then run `docker run --rm --name locald-<project>-<service> -p <host_port>:<container_port> ...`.
- [ ] Ensure `stop()` kills the docker CLI process (and maybe runs `docker stop` explicitly if needed).

### Step 3: Verification
- [ ] Create a test project with a `postgres` service.
- [ ] Verify it starts and binds a port.
- [ ] Verify `locald logs` shows postgres logs.
- [ ] Verify `locald stop` cleans up the container.
