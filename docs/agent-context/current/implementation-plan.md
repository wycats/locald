# Phase 12 Implementation Plan: Docker Integration

## Goal

Unified lifecycle for local apps and Docker containers. The user should be able to define a service as a Docker image in `locald.toml`, and `locald` should manage it just like a local process.

## User Requirements

- **App Builder**: Wants to spin up a database (e.g., Postgres, Redis) or a backend service (e.g., Keycloak) without installing it on their machine.
- **Power User**: Wants to configure container ports, environment variables, and volumes.
- **Contributor**: Wants a clean abstraction for "Runners" (Process vs Container).

## Strategy

1.  **Schema Extension**: Add `image` and `container_port` to `ServiceConfig`.
2.  **Abstraction**: Refactor `ProcessManager` into a trait or enum that can handle both `Process` and `Container` workloads. Let's call it `WorkloadManager` or just keep `ProcessManager` but make it smarter.
    - _Better approach_: Create a `Runtime` trait? Or just an enum `ServiceType { Process, Container }`.
    - Let's keep it simple: `ProcessManager` currently manages _processes_. We might need a `ContainerManager`.
    - The `Service` struct in `locald-server` will hold either a `Child` (process) or a `ContainerId`.
3.  **Docker Interaction**: Use the `bollard` crate for robust, async interaction with the Docker Daemon.
    - Fallback: If `bollard` is too heavy, wrap the `docker` CLI. (Decision: Start with `bollard` research).
4.  **Networking**:
    - `locald` assigns a host port (as usual).
    - We map `host_port:container_port` when starting the container.
    - We proxy traffic to `localhost:host_port`.

## Step-by-Step Plan

### Step 1: Research & Design

- [ ] Research `bollard` crate capabilities and compile-time impact.
- [ ] Design the `ServiceConfig` changes.
- [ ] Design the `Runtime` abstraction (Process vs Container).

### Step 2: Core Changes

- [ ] Update `locald-core` with `image` and `container_port` fields.
- [ ] Update `locald.toml` parser.

### Step 3: Docker Implementation

- [ ] Add `bollard` dependency.
- [ ] Implement `ContainerRuntime` (start, stop, logs).
  - `start`: Pull image (if needed), create container, start container.
  - `stop`: Stop and remove container.
  - `logs`: Stream logs from Docker API.

### Step 4: Integration

- [ ] Integrate `ContainerRuntime` into the main loop.
- [ ] Ensure `depends_on` works with containers (wait for them to be "running").

### Step 5: Verification

- [ ] Create a `redis` example in `examples/`.
- [ ] Verify `locald start` brings up the container.
- [ ] Verify `locald stop` tears it down.
