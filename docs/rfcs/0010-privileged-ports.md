---
title: "Privileged Ports: Capabilities over Root"
stage: 3
feature: Architecture
---

# RFC: Privileged Ports: Capabilities over Root

## 1. Summary

The daemon shall run as the user, but use `locald-shim` (setuid root) to bind to privileged ports (80/443) and pass file descriptors back to the unprivileged daemon.

## 2. Motivation

We want to bind port 80 for clean URLs, but running the entire daemon as root violates the principle of least privilege and Axiom 04 (Process Ownership).

## 3. Detailed Design

Privileged port binding is performed by the setuid-root `locald-shim`, keeping the daemon unprivileged.

This avoids granting extra privileges (capabilities) to the main `locald` binary and keeps all privileged operations centralized in the shim.

### Terminology

- **Capabilities**: Linux capabilities (fine-grained privileges).
- **cap_net_bind_service**: The capability to bind to privileged ports.
- **Shim**: `locald-shim`, a small setuid-root helper for privileged operations.
- **SCM_RIGHTS**: POSIX mechanism for passing file descriptors over Unix domain sockets.

### User Experience (UX)

Users run `sudo locald admin setup` once to install and configure `locald-shim` (owned by root with the setuid bit set).

After setup, the daemon can request privileged ports by invoking the shim as-needed.

### Cross-Platform Support

**Implementation Decision**: The shim-based approach works on both Linux and macOS.

| Platform | Setuid Support | FD Passing | Status     |
| -------- | -------------- | ---------- | ---------- |
| Linux    | ✅ Native      | SCM_RIGHTS | ✅ Working |
| macOS    | ✅ Native      | SCM_RIGHTS | ✅ Working |

Both platforms support:
- Setuid binaries with the same semantics
- Unix domain sockets with `SCM_RIGHTS` for file descriptor passing
- The same `locald-shim bind <port>` command interface

This provides **full UX parity** for privileged port binding across platforms.

### Architecture

N/A

### Implementation Details

- `locald-shim bind <port>` binds the privileged port and passes the FD back to the daemon via `SCM_RIGHTS` over a Unix domain socket.
- The daemon uses the received listener for the proxy and remains an unprivileged process.
- The same mechanism works on both Linux and macOS (POSIX-standard).

## 4. Drawbacks

- Requires `sudo` for setup.
- The shim is a privileged binary and must stay minimal and well-audited.

## 5. Alternatives

- Run as root (insecure).
- Port forwarding (iptables).
- `authbind`.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- If needed, restrict which privileged operations the shim supports even further.
- Consider additional hardening (seccomp/apparmor) around the shim.
