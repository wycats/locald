---
title: "Daemon Lifecycle: PID File & Socket Safety"
stage: 3
feature: Architecture
---

# RFC: Daemon Lifecycle: PID File & Socket Safety

## 0. Current Implementation (Reality)

This RFC describes a more ambitious lifecycle-safety protocol. The current implementation covers several of the core ideas, but differs in details:

- **Socket-first idempotency**: On startup, the daemon checks whether it can connect to the IPC socket. If it can, it exits successfully (already running). If the socket path exists but is stale, it removes it before binding.
- **CLI bootstrap**: Commands typically "ping then spawn". If the daemon is not reachable, the CLI spawns a detached daemon via `setsid locald server start` and waits for it to respond.
- **Version mismatch restart**: The CLI uses an IPC version request to detect mismatches and may shut down and restart the daemon.
- **Sandbox socket path**: Sandboxed runs set `LOCALD_SOCKET` and require `LOCALD_SANDBOX_ACTIVE=1`.

The manual is authoritative for current behavior:

- [Architecture: Core](../manual/architecture/core.md)

## 1. Summary

Introduce a robust startup protocol for the `locald` daemon using a PID file and smarter socket handling to prevent "zombie socket" loops and provide clear error messages when the daemon is in an inconsistent state.

## 2. Motivation

Historically, one failure mode was blindly overwriting the Unix domain socket (`/tmp/locald.sock`) on startup. This leads to a failure mode where:

1.  A valid `locald` process is running and holding port 80.
2.  A new `locald` instance starts (e.g., triggered by `locald up`), overwrites the socket file, but fails to bind port 80 because the old process holds it.
3.  The new instance crashes.
4.  The CLI tries to connect to the _new_ (now dead) socket, fails, and assumes the daemon is down.
5.  The user is stuck in a loop where `locald up` fails repeatedly, and the actual running daemon is unreachable.

We need a way to detect if a daemon is truly running, even if the socket file is missing or stale, and recover gracefully or inform the user.

## 3. Detailed Design

### 3.1. The PID File

This section proposes a PID-file-based protocol. The daemon includes a PID file in one daemonization mode, but the primary bootstrap path relies on socket health checks and `setsid` detachment.

**Startup Logic:**

1.  **Check PID File**:

    - Read the PID from the file.
    - Check if a process with that PID exists (`kill(pid, 0)`).
    - _Optional_: Check if the process name is `locald`.
    - **If running**: Abort startup. Print: "locald is already running (PID: <pid>)."
    - **If not running**: The PID file is stale. Delete it and proceed.

2.  **Check Socket**:

    - If `locald.sock` exists:
      - Try to connect to it.
      - **If connection succeeds**: Abort. "locald is already running and listening."
      - **If connection refused**: The socket is stale. Unlink it and proceed.

3.  **Bind Port (The Final Guard)**:
    - Attempt to bind the server port (e.g., 80).
    - **If successful**: Write new PID file. Start serving.
    - **If EADDRINUSE**:
      - This implies another process holds the port, but we didn't detect it via PID file or Socket.
      - **Action**: Fail with a descriptive error: "Port 80 is in use. Please check for zombie locald processes or other web servers."

### 3.2. CLI `up` Logic

The CLI's `up` command should be robust in the face of stale sockets and version mismatches:

1.  Try to connect to socket.
2.  If successful -> Done.
3.  If "Connection Refused" (Socket exists but no listener) OR "NotFound" (No socket):
    - Check for PID file.
    - If PID file exists and process is running:
      - **Error**: "Daemon is running (PID <pid>) but unreachable. The socket file may be corrupted or the daemon is hung."
      - **Hint**: Prefer `locald server shutdown` (or follow manual troubleshooting steps).
    - If no PID file (or stale):
      - Spawn new daemon.

### 3.3. Version & Binary Consistency (Self-Repair)

A common development pitfall occurs when a user rebuilds `locald` (e.g., `cargo build`) but the CLI connects to an old, already-running daemon (e.g., installed via `cargo install`). This leads to confusing behavior where code changes appear ineffective.

**The Handshake:**

1.  **Identity Check**: On every command, the CLI sends an identity/version request.

- Current implementation uses `IpcRequest::GetVersion`.

2.  **Daemon Response**: The daemon replies with:

- `version`: a build/version string.
- (Future) `binary_path` and `build_hash` can be added if needed.

3.  **Verification**:
    - The CLI compares the daemon's `binary_path` with its own `current_exe()`.
    - The CLI compares the `version`.
4.  **Auto-Heal**:
    - If a mismatch is detected, the CLI prints a warning: "Daemon version mismatch (Running: X, CLI: Y). Restarting..."
    - The CLI sends `IpcRequest::Shutdown`.
    - The CLI waits for the socket to disappear.
    - The CLI spawns the new daemon (itself).
    - The CLI retries the original command.

### 3.4. Cleanup

- **Graceful Shutdown**: The daemon must remove the PID file and Socket file on `SIGTERM`/`SIGINT`.
- **Crash Handling**: If the daemon crashes, the PID file remains. The startup logic (Step 1) handles this by verifying the PID is alive.

## 4. Implementation Plan (Stage 2)

- [ ] Implement `PidFile` struct in `locald-server` (create on start, remove on drop).
- [ ] Update `locald-server` startup sequence to check PID file and Socket before binding.
- [ ] Implement `IpcRequest::GetServerInfo` (or similar richer identity response) in `locald-core` and `locald-server`.
- [ ] Update `locald-cli` to perform the "Identity Check" handshake on startup.
- [ ] Implement auto-restart logic in `locald-cli`.
- [ ] Update `locald-cli` `up` command to check PID file before spawning.
- [ ] Add integration test for "zombie" scenario (start daemon, delete socket, try to start again).

## 5. Context Updates (Stage 3)

List the changes required to `docs/agent-context/` to reflect this feature as "current reality".

- [ ] Update `docs/agent-context/architecture/daemon-lifecycle.md` (create if needed).
- [ ] Update `docs/agent-context/plan-outline.md` to mark this work as complete.

## 6. Drawbacks

- **Complexity**: Adds state management (PID file) that can get out of sync.
- **Stale Locks**: If the OS reuses PIDs rapidly (unlikely but possible), we might think a valid process is our daemon. Checking process name helps.

## 6. Alternatives

- **Socket Lock**: Use `flock` on the socket file itself. This is cleaner but can be tricky with Unix sockets that get unlinked.
- **Port Check Only**: Just try to bind the port. If it fails, assume running. This is insufficient because we need to know _who_ holds the port to give good error messages, and we need to handle the "socket is wrong" case.

## 7. Unresolved Questions

- Should we automatically kill the zombie process if we detect it?
  - _Decision_: No. Explicit is better than implicit. Tell the user.
