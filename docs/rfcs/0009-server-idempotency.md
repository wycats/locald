---
title: "Server Idempotency: Socket Check"
stage: 3
feature: Architecture
---

# RFC: Server Idempotency: Socket Check

## 1. Summary

The server binary shall check if the IPC socket is already active before starting.

## 2. Motivation

Running `locald-server` multiple times shouldn't cause errors or zombie processes. We need to ensure only one instance runs at a time.

## 3. Detailed Design

Before binding to the socket, the server attempts to connect to it. If the connection succeeds, it means another instance is running, so it exits. If the connection fails (Connection Refused), it assumes the socket is stale (or not there) and proceeds (cleaning up the stale file if necessary).

### Terminology

- **Idempotency**: The property that an operation can be applied multiple times without changing the result beyond the initial application.

### User Experience (UX)

Running `locald server start` when it's already running is a no-op (or prints a message).

### Architecture

Startup logic in `locald-server`.

### Implementation Details

Connect to the Unix Domain Socket.

## 4. Drawbacks

- Race conditions (small window).

## 5. Alternatives

- PID file locking.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
