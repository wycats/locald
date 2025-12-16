---
title: "IPC: Unix Domain Sockets"
stage: 3
feature: Architecture
---

# RFC: IPC: Unix Domain Sockets

## 1. Summary

The CLI and Daemon shall communicate using Unix Domain Sockets.

## 2. Motivation

We need a low-latency, reliable, and secure way for the CLI to send commands to the local daemon. Unix Domain Sockets are standard on POSIX systems and offer better security than TCP sockets (file permissions).

## 3. Detailed Design

The daemon listens on a socket file (e.g., `/tmp/locald.sock`). The CLI connects to this socket to send commands.

### Terminology

- **IPC**: Inter-Process Communication.
- **Socket**: The Unix Domain Socket file.

### User Experience (UX)

Transparent to the user.

### Architecture

The protocol will be newline-delimited JSON.

### Implementation Details

Use `tokio::net::UnixListener` and `tokio::net::UnixStream`.

## 4. Drawbacks

- Not supported on Windows (historically, though now available).
- Requires managing the socket file (cleanup).

## 5. Alternatives

- TCP/IP (localhost).
- Named Pipes.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- Support for Windows Named Pipes if we port to Windows.
