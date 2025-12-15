---
title: "Architecture: Security"
---

This document describes the security architecture of `locald`, focusing on privilege separation and capability management.

## 1. Privilege Separation (The Shim)

`locald` aims to run with the least privilege necessary. However, binding to privileged ports (80 and 443) typically requires root access. To solve this, we use a **Shim Architecture**.

- **`locald-shim`**: A small, root-owned (setuid) binary. Its jobs are:
  1.  Bind privileged ports (80/443) and pass the file descriptor to the daemon.
  2.  Execute OCI bundles with root privileges using an embedded container runtime, while enforcing user mapping to protect the host filesystem.
- **`locald-server`**: The main daemon, running as the normal user.

### Shim Versioning

To ensure safety and compatibility, the daemon and the shim perform a strict version handshake.

- The daemon checks the shim's version on startup.
- If the shim is outdated, the daemon refuses to start privileged operations and instructs the user to update (via `sudo locald admin setup`).

## 2. Container Execution

Executing containers securely requires a delicate balance. We need root privileges to set up namespaces (Mount, PID, Network) and Cgroups, but we want the process inside the container to run as the user (or map to the user) to prevent file permission issues on the host.

We achieve this by routing container execution through the `locald-shim`.

- **Command**: `locald-shim bundle run --bundle <PATH> --id <ID>`
- **Security**: The shim is the privileged leaf node. It executes the OCI bundle using an embedded runtime and applies user mapping (via `config.json`), so container processes can run with container-root semantics while files on the host remain owned by the unprivileged user.

## 3. Capabilities

On Linux, we leverage **Capabilities** (`cap_net_bind_service`) to allow binding to low ports without full root privileges.

- The `locald-shim` (or the main binary in simple setups) is granted this capability via `setcap`.
- This is preferred over running the entire daemon as root.

## 3. Socket Security

The Unix Domain Socket used for IPC is secured via file permissions. Only the user who started the daemon (and root) can read/write to the socket, preventing other users on the system from controlling the daemon.
