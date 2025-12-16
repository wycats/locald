# Architecture: Resource Management (Cgroups)

This page describes the cgroup v2 structure that `locald` is implementing to ensure reliable lifecycle management and resource accounting. This structure is known as the **"Tree of Life"**.

## Current Status

As of 2025-12-16, the “Tree of Life” is not yet fully enforced.

- OCI spec generation supports `linux.cgroupsPath`, but most bundle generation paths currently leave it unset.
- Service stop/restart is primarily PID/signal-based, which cannot guarantee cleanup for double-fork subprocess trees.

Phase 99 (RFC 0099) completes the missing wiring and kill semantics.

## The Hierarchy

The hierarchy mirrors the logical structure of `locald`'s runtime, ensuring that every process belongs to a specific service and sandbox.

```text
/sys/fs/cgroup/
  ├── locald.slice/                  # Global Root
  │     ├── locald-<sandbox>.slice/  # Sandbox Root (e.g., "default", "test")
  │     │     ├── service-<name>.scope/ # Service Leaf
  │     │     │     ├── cgroup.procs   # PIDs
  │     │     │     └── ...
```

### Naming Convention

- **Global Root**: `locald.slice`. This is the top-level anchor.
- **Sandbox Root**: `locald-<sandbox>.slice`. Isolates different `locald` instances or environments (e.g., `locald-test.slice`).
- **Service Leaf**: `service-<name>.scope`. The actual container runs here. We use `.scope` because these are transient units managed programmatically, not static systemd services.

## The Anchor & The Driver

`locald` must establish ownership of `locald.slice` before it can manage resources. It uses two strategies depending on the host environment.

### Strategy A: The Anchor (Systemd)

If the host uses Systemd (detected by the presence of `/run/systemd/system`), `locald` behaves as a polite tenant.

1.  **Lease**: The `locald-shim` (via `admin setup`) writes a Unit File to `/etc/systemd/system/locald.slice`.

    ```ini
    [Unit]
    Description=Locald Container Runtime Root
    Documentation=https://github.com/wycats/dotlocal

    [Slice]
    Delegate=yes
    ```

2.  **Delegation**: The `Delegate=yes` directive instructs Systemd to grant `locald` full management rights over the cgroup subtree. Systemd will not interfere with processes or controllers inside this slice.

### Strategy B: The Driver (Direct/Fallback)

If Systemd is absent (e.g., inside a specialized container or minimal VM), `locald` manages the cgroup filesystem directly.

1.  **Create**: `mkdir -p /sys/fs/cgroup/locald`.
2.  **Enable Controllers**:
    - Read available controllers from `/sys/fs/cgroup/cgroup.controllers`.
    - Write them (e.g., `+cpu +memory`) to `/sys/fs/cgroup/cgroup.subtree_control`.
    - This "activates" the controllers for the root.
3.  **Propagate**: Repeat the enabling process for `/sys/fs/cgroup/locald/cgroup.subtree_control` to ensure nested slices can use resources.

## Runtime Integration

`locald` uses `libcontainer` (via `locald-shim`) to execute containers.

1.  **Generator**: `locald-server` calculates the absolute path (e.g., `/locald.slice/locald-default.slice/service-web.scope`) and writes it to the `linux.cgroupsPath` field in the OCI `config.json`.
2.  **Executor**: `locald-shim` reads this path. It ensures the directory structure exists (creating parent slices if needed) and moves the process into the leaf cgroup _before_ execution.

## Lifecycle: "Scorched Earth"

The hierarchy allows `locald` to implement a robust kill strategy.

1.  **Graceful**: Send `SIGTERM` to the main PID.
2.  **Forceful**: `locald` targets the **Cgroup**, not just the PID.
    - It writes `1` to `cgroup.kill` (if available).
    - Or it freezes the cgroup, kills all PIDs in `cgroup.procs`, and thaws.
3.  **Cleanup**: The empty cgroup directories are removed.

This guarantees that no orphaned subprocesses (double-forks) survive a service restart.

## Verification

Verification is easiest on a Linux host with cgroup v2 enabled.

1.  Ensure the cgroup root is configured:
    - `sudo locald admin setup`
2.  Start a project that uses container execution (CNB or container services).
3.  Inspect the hierarchy:
    - `systemd-cgls` (on systemd hosts): confirm the `locald.slice` subtree contains per-sandbox slices and per-service scopes.
    - Or inspect `/sys/fs/cgroup` directly.
4.  Stop/restart the service and confirm cleanup:
    - Leaf `cgroup.procs` becomes empty.
    - The leaf directory is pruned after stop.

For debugging, you can also invoke the privileged cleanup directly:

- `sudo locald-shim admin cgroup kill --path /locald.slice/locald-default.slice/service-web.scope`
