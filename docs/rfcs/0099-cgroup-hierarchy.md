# RFC 0099: Structured Cgroup Hierarchy ("Tree of Life")

## Status

- **Stage**: 3 (Recommended)
- **Created**: 2025-12-12
- **Manual**: `docs/manual/architecture/resource-management.md`

## Implementation Status

As of 2025-12-16, this RFC is implemented with a safety gate:

- `locald-shim` already executes OCI bundles via embedded `libcontainer`.
- The OCI generator supports `linux.cgroupsPath`.
- `locald-server` computes per-sandbox/per-service cgroup paths and wires them into bundle generation (gated on a configured cgroup root).
- Service stop now triggers a cgroup-level kill (via `locald-shim`) when a cgroup path is available, ensuring cleanup even for double-forked subprocess trees.

Phase 99 implements the remaining pieces.

## Context

Currently, `locald` delegates cgroup management entirely to the OCI runtime (currently `runc`, moving to `libcontainer`). By default, this results in a "Wild West" scenario where processes are spawned in the user's slice or an ad-hoc scope, with no predictable hierarchy.

This lack of structure causes two critical issues:

1.  **Lifecycle Leaks**: We cannot reliably kill a service and _all_ its children. If a service spawns a subprocess and double-forks, `locald` loses track of it.
2.  **Resource Anarchy**: We cannot measure or limit resources per service or for the entire `locald` instance.

## Decision

We will implement a strict, hierarchical Cgroup v2 structure managed by `locald`.

### Runtime Strategy: No More `runc`

Upon completion of this RFC, `locald` will exclusively use the embedded `libcontainer` library (via `locald-shim`) for container execution.

- **Removal**: The `runc` binary dependency is permanently removed.
- **Prohibition**: We shall not re-introduce `runc` or any external OCI runtime binary. The "Fat Shim" architecture (RFC 0098) is the sole execution path.

### The Hierarchy

The hierarchy will mirror the logical structure of `locald`'s runtime:

```text
/sys/fs/cgroup/
  ├── locald.slice/                  # Global Root
  │     ├── locald-<sandbox>.slice/  # Sandbox Root (e.g., "default", "test")
  │     │     ├── service-<name>.scope/ # Service Leaf
  │     │     │     ├── cgroup.procs   # PIDs
  │     │     │     └── ...
```

### Components

1.  **Admin Setup (The Anchor & The Driver)**:

    - The `locald-shim` (via `admin setup`) will detect the environment and establish the root.
    - **Strategy A: The Anchor (Systemd)**:
      - **Detection**: PID 1 is systemd (`/proc/1/comm` is `systemd`).
        - Note: `/run/systemd/system` is not sufficient on its own (common CI/container false positives).
      - **Action**: Write `/etc/systemd/system/locald.slice` with `Delegate=yes` and reload systemd.
      - **Result**: Systemd grants us `/sys/fs/cgroup/locald.slice` with full delegation.
    - **Strategy B: The Driver (Direct/Fallback)**:
      - **Detection**: PID 1 is not systemd.
      - **Prerequisite**: `/sys/fs/cgroup` must be a cgroup2 filesystem.
      - **Action**:
        1.  `mkdir -p /sys/fs/cgroup/locald`
        2.  **Delegation**: Read `/sys/fs/cgroup/cgroup.controllers`. Write these controllers (prefixed with `+`) to `/sys/fs/cgroup/cgroup.subtree_control`. This enables controllers for our root.
        3.  **Propagation**: Write the same controllers to `/sys/fs/cgroup/locald/cgroup.subtree_control` to allow further nesting.
      - **Result**: We own `/sys/fs/cgroup/locald` and have manually enabled controllers.

2.  **Server (Generator)**:

    - The `locald-server` is responsible for calculating the absolute cgroup path for each service.
    - Format: `/locald.slice/locald-<sandbox>.slice/service-<name>.scope` (or `/locald/...` if no systemd).
    - This path is passed to the OCI Generator.

3.  **OCI Spec (Contract)**:

    - The `linux.cgroupsPath` field in `config.json` will be populated with the calculated path.
    - This makes the hierarchy explicit in the OCI bundle, decoupling the "what" (policy) from the "how" (execution).

4.  **Shim (Executor)**:
    - The `locald-shim` (via `libcontainer`) will read `linux.cgroupsPath`.
    - It will ensure the cgroup exists (creating parent slices if necessary) and move the process into it _before_ execution.
    - **Constraint**: The shim must have permissions to write to `/sys/fs/cgroup`. As a setuid root binary, it does.

### Lifecycle: "Scorched Earth"

To stop a service, we will no longer rely solely on `SIGTERM` to the PID.

1.  **Signal**: Send `SIGTERM` to the main PID (as before) to allow graceful shutdown.
2.  **Kill**: If the service does not exit, or to ensure cleanup, we use the cgroup to kill _everything_.
    - **Method**: Write `1` to `cgroup.kill` (Linux 5.13+).
  - **Fallback**: Recursively enumerate `cgroup.procs` in the subtree and `SIGKILL` PIDs (best-effort, intentionally conservative; no freezer semantics).
3.  **Prune**: Remove the empty cgroup directory.

  ## Acceptance Criteria (Phase 99)

  This RFC is considered complete when:

  1. **Root ownership is established**
    - `locald admin setup` (via the shim) establishes either:
      - Systemd Anchor: `/sys/fs/cgroup/locald.slice` exists and is delegated, or
      - Driver fallback: `/sys/fs/cgroup/locald` exists and has controllers enabled for nesting.

  2. **Deterministic hierarchy is applied at runtime**
    - Every shim-run service is placed in a leaf cgroup at:
      - Systemd: `/locald.slice/locald-<sandbox>.slice/service-<name>.scope`, or
      - Driver: `/locald/locald-<sandbox>/service-<name>`.
    - `<sandbox>` and `<name>` are sanitized into safe cgroup path components (e.g. `:` and other delimiters map to `-`; `..` and empty components are not permitted).
    - The computed absolute path is written to `linux.cgroupsPath` in the OCI bundle `config.json`.

  3. **Stop/restart guarantees cleanup**
    - `locald stop` and `locald restart` use cgroup-level kill semantics after a grace period.
    - No leaked subprocesses remain after stop/restart, including double-fork cases.

  4. **Verification is documented**
    - Manual verification instructions exist (e.g. using `systemd-cgls` or filesystem inspection under `/sys/fs/cgroup`).

## Consequences

### Positive

- **Guaranteed Cleanup**: No more zombie processes or leaked subprocesses.
- **Isolation**: Services are strictly isolated in the kernel's view.
- **Observability**: We can read `cpu.stat` or `memory.current` for the cgroup to get accurate metrics for the _entire_ service tree.

### Negative

- **Complexity**: Requires `locald-server` to understand cgroup paths.
- **Privilege**: Requires the shim to manage cgroup directories (already covered by setuid).
- **Kernel Dependency**: Requires Cgroup v2 (standard on modern Linux, but excludes very old kernels).

## Implementation Plan

1.  **Update OCI Generator**: Add `cgroup_path` support (Done).
2.  **Update Server**: Implement `CgroupManager` to generate paths based on sandbox and service name.
3.  **Update Shim**: Verify `libcontainer` creates the hierarchy correctly.
4.  **Verify**: Use `systemd-cgls` to visualize the tree during a test.
