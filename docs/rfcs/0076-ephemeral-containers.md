# RFC 0076: Ephemeral Containers & The "Run" Workflow

## Status

- **Status**: Recommended
- **Date**: 2025-12-08

## Reality Check (Current Implementation)

This RFC captures the original "north star" framing. The implementation has since converged on a slightly different shape:

- **IPC**: CLI ↔ daemon communication is Unix Domain Socket IPC (JSON), not gRPC.
- **Execution**: `locald-shim` executes OCI bundles via embedded `libcontainer` (the Fat Shim model), not by spawning an external `runc` binary.
- **Orchestration**: The daemon orchestrates pull → unpack → spec generation → shim execution.

For current behavior, treat the manual as the source of truth:

- [Ephemeral Containers](../manual/features/ephemeral-containers.md)
- [Container Runtime](../manual/architecture/container-runtime.md)
- [Shim Management](../manual/architecture/shim-management.md)

## Vision

To validate the architectural completeness of `locald` by ensuring its components (`locald-oci`, `locald-shim`, `locald-server`) can compose to support a `docker run`-style workflow. While we may not expose this exact CLI surface immediately, the ability to pull, unpack, configure, and execute an arbitrary OCI image is the ultimate integration test for our platform's core abstractions.

## Motivation

We are building a platform, not just a process manager. To ensure our abstractions are robust, we need a "North Star" use case that stresses every part of the system. Implementing the mechanics of `docker run` (ad-hoc container execution) forces us to solve:

1.  **OCI Image Management**: Pulling and unpacking layers efficiently.
2.  **Runtime Spec Generation**: Correctly translating image config to runtime specs.
3.  **Process Supervision**: Managing the lifecycle of a containerized process.
4.  **Interaction**: Handling TTYs and signals correctly.

If we can do this, we prove that our "local development platform" is built on a foundation as capable as the industry standard, even if our user-facing workflow differs. We don't need bug-for-bug parity with Docker, but we need to know our core composes.

## Architecture

### 1. The "Pull-and-Run" Pipeline

We need a unified pipeline in `locald-server` that orchestrates the transition from "Image Name" to "Running Process".

```mermaid
graph TD
  A[User: locald container run ubuntu] -->|UDS IPC (JSON)| B(locald daemon)
    B -->|1. Pull| C[locald-oci: Registry]
    C -->|2. Unpack/Snapshot| D[Filesystem Bundle]
    B -->|3. Generate Spec| E[config.json]
    B -->|4. Execute| F[locald-shim]
  F -->|5. Execute| G[libcontainer]
```

### 2. Component Responsibilities

#### `locald-oci` (Expanded Scope)

- **Current**: Handles Image Spec (Manifests, Blobs, Layouts).
- **New**:
  - **Snapshotting**: Efficiently creating a bundle rootfs.
    - _Phase 1_: Naive copy (already exists).
    - _Phase 2_: Hardlink farm (fast, space-efficient).
    - _Phase 3_: OverlayFS (if supported) or FUSE.
  - **Spec Generation**: Converting `OciImageConfig` -> `OciRuntimeSpec`.
    - Mapping `Env`, `Entrypoint`, `Cmd`, `WorkingDir`.
    - Handling User/Group mappings (rootless support).
  - **Runtime Interface**: A Rust wrapper around invoking `locald-shim` for bundle execution.

#### `locald-shim`

- **Role**: The Supervisor.
- **Responsibility**:
  - Receives the path to the Bundle (prepared by Server/OCI).
  - Invokes `locald_oci::runtime::run`.
  - **TTY Proxying**: For interactive sessions (`-it`), the shim must bridge the PTY master to the Server, which bridges it to the CLI.

#### `locald-server`

- **Role**: The Orchestrator.
- **Responsibility**:
  - Manages the "Container Store" (where bundles live).
  - Handles the lifecycle of ephemeral containers (auto-cleanup).

### 3. The User Experience

Since `locald run <service>` is already taken (Task Mode, RFC 0032), we will introduce a new top-level command group `container` for raw container operations.

```bash
# Run an interactive shell
locald container run -it ubuntu:latest bash

# Run a detached background task
locald container run -d postgres:15

# Run with mapped ports
locald container run -p 8080:80 nginx
```

## Implementation Plan

### Phase 1: The Foundation (Current Epoch)

1.  **Spec Generation**: Implement `locald_oci::runtime_spec::generate(image_config) -> Spec`.
2.  **Runtime Wrapper**: Implement `locald_oci::runtime::run(bundle_path)`.
3.  **Integration**: Update `locald-shim` to use these new `locald-oci` capabilities instead of raw `Command::new("runc")`.

### Phase 2: The Pipeline

1.  **Server Logic**: Add `ContainerService` to `locald-server`.
2.  **CLI Command**: Add `locald container run` (distinct from `locald run`).

### Phase 3: Interactivity

1.  **PTY Support**: Implement raw terminal handling in CLI and PTY bridging in Shim.

## Package Strategy & Testing

We will maintain the current package structure, ensuring strict boundaries to allow for targeted usage.

### 1. `locald-oci`

- **Scope**: The "Container Engine Library".
  - **Responsibilities**: Registry interactions, Image Layout management, Runtime Spec generation, and Runtime invocation (wrapping `runc`).
  - **Why**: A user building a custom tool to run a container should only need this crate.
- **Testing Strategy**:
  - **Unit**: Extensive tests for `OciImageConfig` -> `OciRuntimeSpec` conversion (e.g., ensuring env vars are merged correctly).
  - **Integration**:
    - `test_pull_and_unpack`: Mock a registry, pull a small image (e.g., `alpine`), unpack it, verify filesystem.
    - `test_runtime_invoke`: Create a dummy bundle and invoke `runc` to run `echo hello`.

### 2. `locald-shim`

- **Scope**: The "Supervisor Process".
  - **Responsibilities**: Process lifecycle, signal forwarding, IO streaming, TTY bridging.
  - **Why**: It isolates the server from the child process. It is a _consumer_ of `locald-oci`.
- **Testing Strategy**:
  - **Integration**: Spawn the shim binary with a dummy command (e.g., `sleep 1`). Send signals (SIGINT) and verify the child receives them. Verify stdout is captured.

### 3. `locald-server`

- **Scope**: The "Orchestrator".
  - **Responsibilities**: API, State Management, High-level "Run" pipeline.
- **Testing Strategy**:
  - **E2E**: Use the CLI to run `locald run alpine echo hello`. Verify the command succeeds and output is returned. This tests the entire chain.

### 4. `locald-core` & `locald-utils`

- **Scope**: Shared types and low-level system primitives.
- **Testing Strategy**: Unit tests for all utility functions (e.g., signal handling helpers, config parsing).

## Decision Log

- **Where does `runc` logic live?**: In `locald-oci`. It is the "OCI Runtime" library. `locald-shim` is the consumer/supervisor.
- **Package Consolidation**: We will NOT create a new `locald-runtime` crate yet. `locald-oci` will handle both the "Image" and "Runtime" aspects of OCI, as they are tightly coupled via the Spec. If dependencies (network vs process) become an issue, we will use feature flags (`feature = "registry"`, `feature = "runtime"`).
