# Implementation Plan - Phase 76: Ephemeral Containers

**Goal**: Validate the architectural completeness of `locald` by ensuring its components (`locald-oci`, `locald-shim`, `locald-server`) can compose to support a `docker run`-style workflow.
**RFC**: [docs/rfcs/0076-ephemeral-containers.md](../../rfcs/0076-ephemeral-containers.md)

## 1. OCI Library Enhancement (locald-oci)

Expand `locald-oci` to be a true "Container Engine Library".

- **Spec Generation**: Implement logic to convert an `OciImageConfig` (from the image manifest) into an `OciRuntimeSpec` (`config.json`). This involves mapping environment variables, entrypoints, and working directories.
- **Runtime Interface**: Define the interface for invoking the OCI runtime. (Note: Implementation details moved to `locald-shim` via RFC 0098).

## 2. Shim Integration (locald-shim)

Update the supervisor process to use the new library capabilities.

- **Refactor (RFC 0098)**: Pivot from wrapping `runc` CLI to embedding `libcontainer`.
  - **Fat Shim**: `locald-shim` now acts as the OCI Runtime itself.
  - **Interface**: Accepts a bundle path, loads `config.json`, and executes the container directly using `libcontainer`.

## 3. Server Orchestration (locald-server)

Implement the "Pull-and-Run" pipeline in the server.

- **ContainerService**: Add a new service component to manage the lifecycle of ephemeral containers.
- **Pipeline**: Implement the flow:
  1.  **Pull**: Fetch image from registry (using existing `locald-oci` logic).
  2.  **Unpack**: Create a filesystem bundle.
  3.  **Generate Spec**: Create `config.json` using the new `locald-oci` logic.
  4.  **Execute**: Spawn `locald-shim` pointing to the bundle.

## 4. CLI Implementation (locald-cli)

Expose the functionality to the user.

- **Command**: Add `locald container run` command group.
- **UX**: Support basic flags like `-d` (detach) and `-p` (port mapping).

## 5. Verification

- **E2E**: Verify the full flow using a simple image (e.g., `alpine`).
