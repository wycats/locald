# Architecture: Security

This document describes the security architecture of `locald`, focusing on privilege separation and capability management.

## 1. Privilege Separation (The Shim)

`locald` aims to run with the least privilege necessary. However, binding to privileged ports (80 and 443) typically requires root access. To solve this, we use a **Shim Architecture**.

- **`locald-shim`**: A small, root-owned (setuid) binary. Its jobs are:
  1.  Bind privileged ports (80/443) and pass the file descriptor to the daemon.
  2.  Execute OCI bundles using an embedded `libcontainer` runtime (the "Fat Shim" model).
- **Daemon (server mode)**: The main long-running process, running as the normal user.

### Shim Versioning

To ensure safety and compatibility, `locald` verifies the shim before using privileged features.

- `locald-shim --shim-version` provides a compatibility identifier.
- If the shim is missing or outdated, `locald` refuses to proceed and instructs the user to run `sudo locald admin setup`.

## 2. Container Execution

Executing containers securely requires a delicate balance. We need root privileges to set up namespaces (Mount, PID, Network) and cgroups, but we want the process inside the container to map back to the user to avoid host permission issues.

We achieve this by routing container execution through the `locald-shim`.

- **Command**: `locald-shim bundle run --bundle <path-to-oci-bundle> --id <id>`
- **Security**: The shim validates inputs and runs the bundle via embedded `libcontainer`. The container is configured (via `config.json`) to use User Namespaces, mapping container users back to the host user.

See [Container Runtime](container-runtime.md) and [Shim Management](shim-management.md) for the detailed execution flow and development workflow.

## 3. Capabilities

On Linux, it is possible to use **Capabilities** (`cap_net_bind_service`) to allow binding to low ports without full root privileges.

- Today, `locald` prefers the setuid shim for privileged operations.
- The daemon itself remains unprivileged.

## 4. Socket Security

The Unix Domain Socket used for IPC is secured via filesystem permissions on the socket path.
