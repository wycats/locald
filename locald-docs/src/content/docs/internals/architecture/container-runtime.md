---
title: "Architecture: Container Runtime"
---

This document describes how `locald` executes processes in isolated environments (containers) using an embedded container runtime (via `locald-shim`) and User Namespaces.

## 1. The "Rootless" Contract

`locald` runs containers without requiring a root daemon (like Docker). It achieves this using **User Namespaces**.

**Important clarification**: In `locald`, “rootless” means **no root daemon** and **no interactive privilege prompts** in the steady-state development loop. Container execution still uses a _privileged leaf node_ (`locald-shim`, setuid root) to perform operations that require elevated privileges (namespaces, mounts, cgroups, privileged ports).

This is distinct from “true rootless containers” (no privileged helper), which depends on host support for unprivileged user namespaces and additional system configuration. `locald` does not assume that configuration is present.

### ID Mapping

We map the **Host User** (the user running `locald`) to **Container Root** (UID 0).

- **Host**: `uid=1000` (ykatz)
- **Container**: `uid=0` (root)

This means processes inside the container _think_ they are root and have full control over the container's filesystem, but on the host, they are just files owned by the normal user.

## 2. Rootfs Requirements

Because `locald` constructs the container filesystem (rootfs) from scratch (or by extracting OCI images), it is responsible for ensuring the rootfs is a **Valid Linux System**.

### The `/etc/passwd` Requirement

Standard Linux tools (shells, `sudo`, language runtimes, and the CNB Lifecycle) rely on `/etc/passwd` and `/etc/group` to resolve user IDs to names, even if those IDs are just mappings.

**Requirement**: The rootfs MUST contain `/etc/passwd` and `/etc/group` files.

If the base image (e.g., a buildpack builder) does not provide them, or if they are lost during extraction, `locald` **MUST** synthesize them. This is not a "hack"; it is a necessary step of **System Provisioning**.

#### Minimal `/etc/passwd` Content

```text
root:x:0:0:root:/root:/bin/sh
cnb:x:1000:1000:cnb:/home/cnb:/bin/sh
```

- `root` (0): Maps to the Host User. Required for the CNB Lifecycle to run as "root" inside the container.
- `cnb` (1000): Often used by buildpacks as the non-root user.

## 3. The Shim Role

`locald` uses the `locald-shim` to execute OCI bundles using an embedded container runtime. This is necessary because creating namespaces and configuring cgroups often requires root privileges, especially on systems with SELinux or restricted user namespaces.

### Execution Flow

1.  **Prepare**: `locald` (User) prepares the OCI bundle (rootfs + config.json).
2.  **Invoke**: `locald` calls `locald-shim bundle run --bundle <path> --id <id>`.
3.  **Privilege Escalation**: `locald-shim` (Setuid Root) validates the command.
4.  **Isolation**: The embedded runtime creates the container namespaces and cgroups.
5.  **User Mapping**: The process switches to the User Namespace, mapping the container's root user (0) to the host user (1000).
6.  **Execution**: The process runs inside the container.

This strategy ("Fat Shim") removes the dependency on an external runtime binary while preserving the same user-mapping goals.

## 4. Standard Filesystem Layout

`locald` guarantees a standard filesystem layout for all containers, compliant with the **Cloud Native Buildpacks (CNB)** specification. This ensures that buildpacks and applications always find files where they expect them.

### `/workspace`

This is the application's source code directory.

- **Source**: The user's project directory (or a copy of it).
- **Usage**: The application is built and run from here.
- **Working Directory**: The container's `cwd` is set to `/workspace`.

### `/layers`

This directory stores buildpack layers, including dependencies and cache.

- **Usage**: Buildpacks write downloaded dependencies (like `node_modules` or JDKs) here.
- **Persistence**: This directory is often mounted as a volume to preserve cache between builds.

### `/cnb`

This directory contains the CNB Lifecycle binaries and configuration.

- **Usage**: The `lifecycle` tool (which orchestrates the build) runs from here.
