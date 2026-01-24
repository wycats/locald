---
title: "Shim Architecture: The Leaf Node Axiom"
stage: 3 # Recommended
feature: Security
---

# RFC 0096: Shim Architecture: The Leaf Node Axiom

## 1. The Axiom

The `locald-shim` binary **MUST NEVER** execute the `locald` binary.

The shim is a **Leaf Node** in the execution graph.

- **Allowed**: `locald` -> `locald-shim`
- **Allowed**: `locald-shim` -> `runc` (or other system tools)
- **FORBIDDEN**: `locald-shim` -> `locald`

This axiom eliminates the possibility of re-execution loops ("fork bombs") by design, rather than by fragile state checks.

## 2. The Problem: The Wrapper Anti-Pattern

The previous architecture treated the shim as a "Privileged Wrapper".

1.  User runs `locald`.
2.  `locald` detects it needs root.
3.  `locald` execs `shim`.
4.  `shim` sets up environment (caps, uid).
5.  `shim` execs `locald` (callback).

**Failure Mode**: If the "inner" `locald` fails to detect it is already privileged (e.g., due to environment sanitization, version mismatch, or logic bug), it will attempt to exec `shim` again. This creates an infinite recursion loop that consumes system resources (PID exhaustion) and freezes the host.

## 3. The Solution: Service-Based Architecture

Instead of wrapping the process, the shim exposes discrete, atomic privileged operations. The main `locald` daemon remains the orchestrator and runs unprivileged for its entire lifecycle.

### 3.1 Port Binding (FD Passing)

**Requirement**: `locald` needs to listen on port 80/443.

**Old Way**: Shim grants `CAP_NET_BIND_SERVICE` to the `locald` process via re-execution.

**New Way**:

1.  `locald` spawns `locald-shim bind --port 80`.
2.  Shim (root) binds the socket.
3.  Shim passes the raw File Descriptor (FD) back to `locald` via `stdout` or a Unix Domain Socket.
4.  Shim exits.
5.  `locald` constructs a `TcpListener` from the raw FD.

### 3.2 Container Management

**Requirement**: `locald` needs to create namespaces/cgroups.

**Old Way**: `locald` calls `shim runc ...`.

**New Way**: Unchanged. This already adheres to the Leaf Node axiom. The shim wraps `runc`, not `locald`.

### 3.3 System Mutation

**Requirement**: `locald` needs to update `/etc/hosts` or clean up root-owned files.

**Old Way**: `locald` calls `shim admin ...`.

**New Way**: Unchanged. The shim performs the operation natively in Rust and exits.

## 4. Implementation Specification

### 4.1 The `bind` Command

```bash
locald-shim bind <port>
```

- **Input**: Port number (u16).
- **Action**:
  1.  Bind TCP listener to `0.0.0.0:<port>`.
  2.  Get the raw FD.
  3.  Send the FD to the parent process.
      - _Mechanism_: `SCM_RIGHTS` over a Unix Domain Socket is the standard way. However, since `locald` spawns the shim, we can potentially use `stdout` if we serialize it, but FDs are process-local.
      - _Refined Mechanism_: `locald` creates a Unix Domain Socket pair. It passes one end to the shim (via inheritance/arg). The shim sends the FD over that socket using `sendmsg` with `SCM_RIGHTS`.
- **Security**:
  - Validate port is in allowed list (optional, or just allow 80/443).
  - Ensure caller is the `locald` user (check uid).

### 4.2 Removal of `server start`

The `server start` command in `locald-shim` MUST be deleted. The shim should no longer contain any logic to `exec` the `locald` binary.

### 4.3 Removal of `LOCALD_SHIM_ACTIVE`

Since there is no re-execution, the `LOCALD_SHIM_ACTIVE` environment variable is no longer needed to prevent recursion. The architecture itself prevents it.

## 5. Migration Strategy

1.  **Shim**: Implement `bind` command using `SCM_RIGHTS`.
2.  **Shim**: Remove `server start` command.
3.  **Daemon**: Update `ProxyManager` to use the shim for binding privileged ports instead of expecting to have capabilities.
    - Try bind directly (fast path).
    - If `EACCES`, spawn shim to get FD.
    - Wrap FD in `TcpListener`.

## 6. Failure Analysis

- **Shim Missing**: `locald` fails to bind port 80. Logs error. No loop.
- **Shim Outdated**: `locald` calls `bind`, shim errors (unknown command). `locald` logs error. No loop.
- **Permission Denied**: Shim fails to bind. Returns error code. `locald` logs error. No loop.

This architecture is robust, deterministic, and fail-safe.
