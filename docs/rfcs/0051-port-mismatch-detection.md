---
title: "Port Mismatch Detection & Negotiation"
stage: 0 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Port Mismatch Detection
---

# RFC: Port Mismatch Detection & Negotiation

## 1. Summary

This RFC proposes a mechanism to detect when a service is listening on a port other than the one assigned by `locald`. It also explores "Socket Activation" as a standardized, zero-config alternative to port assignment.

## 2. Motivation

A common friction point for new users (and forgetful experienced ones) is the "Port Mismatch":
1.  `locald` assigns port `12345` and sets `$PORT=12345`.
2.  The user's app ignores `$PORT` and defaults to `3000`.
3.  `locald` waits for `12345` to be healthy.
4.  The app starts successfully on `3000`.
5.  `locald` times out or reports the service as unhealthy, while the user sees "App started on port 3000" in the logs.

This is confusing. We can detect this state and provide a helpful error message.

## 3. Detailed Design

### 3.1. Port Mismatch Detection (The "Helper")

When a service is starting (especially in `locald try` or `locald run`), we can monitor the process's open sockets.

**Algorithm:**
1.  Spawn the service.
2.  In a background thread, periodically scan for sockets owned by the service's PID tree.
    *   **Linux/macOS**: Scan `/proc/net/tcp` (using the existing `discovery` logic).
    *   **Windows**: Use the IP Helper API (`GetExtendedTcpTable`) to map PIDs to ports.
3.  If we find a listening port `P_found`:
    *   If `P_found == $PORT`: Success! (We already do this for health checks).
    *   If `P_found != $PORT`:
        *   **Warning**: "Service is listening on port 3000, but locald expected 12345."
        *   **Suggestion**: "Please ensure your application respects the `PORT` environment variable."

**UX:**
In `locald try`, this should be a prominent warning or even an interactive prompt:
> "It looks like your app is listening on port 3000. Do you want to configure this service to use a fixed port (3000) instead of a dynamic one?"

### 3.2. Socket Activation (The "De Facto Standard")

While often overlooked, the most robust standard for port negotiation is **Socket Activation** (systemd style).

Instead of assigning a port number and hoping the app binds to it, `locald` can:
1.  Bind the port itself (e.g., `12345`).
2.  Pass the open file descriptor (FD) to the child process.
3.  Set `LISTEN_FDS=1` and `LISTEN_PID=<pid>`.

**Platform Support:**
*   **Linux & macOS**: This works natively on both because they are Unix-based and support File Descriptor inheritance. Even though macOS's `launchd` uses a different API, `locald` (as the parent) can use the `LISTEN_FDS` convention, and apps that support it (Gunicorn, Puma, etc.) will work seamlessly on macOS.
*   **Windows**: While `LISTEN_FDS` is Unix-specific, Windows supports **Handle Inheritance**. Tools like `systemfd` and libraries like `listenfd` implement a protocol for passing socket handles to child processes.
    *   **Alternative**: **Named Pipes** are the native Windows equivalent for local IPC and avoid TCP port conflicts entirely.
    *   **Alternative**: **Unix Domain Sockets** (`AF_UNIX`) are supported on Windows 10 (Build 1803+) and offer a cross-platform, file-based alternative to TCP.

**Benefits:**
*   **Zero Config**: The app doesn't need to parse flags or env vars; it just accepts the connection.
*   **No Race Conditions**: The port is bound before the app starts.
*   **Instant Readiness**: `locald` knows the port is open immediately.

**Drawback**: The app must support systemd-style socket activation. Many web servers (Gunicorn, Puma, etc.) do, but simple scripts might not.

### 3.3. Flag Heuristics (The "Guess")

Can we generalize flags? (e.g., `--port`, `-p`).
There is no universal standard. However, we could implement a "heuristic probe" for `locald try`:

*   If the command fails or listens on the wrong port, `locald` could try to parse the help text (`--help`) to look for `--port` or `-p`.
*   **Conclusion**: This is likely too brittle and complex for the core daemon. It's better to rely on `$PORT` (the de-facto cloud standard) or Socket Activation (the system standard).

## 4. Implementation Plan (Stage 2)

- [ ] **Refactor Discovery**: Move `locald-server/src/discovery.rs` to `locald-core` to share it with the CLI.
- [ ] **Implement Monitor**: Create a `PortMonitor` struct that watches a PID and reports found ports.
    - [ ] Implement Linux/macOS backend (`/proc` or `lsof`).
    - [ ] Implement Windows backend (`GetExtendedTcpTable`).
- [ ] **Integrate into `try`**: Use `PortMonitor` in `locald try` to detect mismatches.
- [ ] **Integrate into `server`**: Use `PortMonitor` in the daemon to provide better error messages in the dashboard/logs.

## 5. Future Possibilities

*   **Socket Activation Support**: Implement `LISTEN_FDS` passing in `locald-server`.
*   **Framework Detection**: Auto-detect frameworks (Rails, Django, Spring) and set their specific port environment variables (`PORT`, `SERVER_PORT`, `ASPNETCORE_URLS`) automatically, reducing the need for the user to know them.

