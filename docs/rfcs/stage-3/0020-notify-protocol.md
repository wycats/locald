---
title: "Notify Protocol: Unix Datagram"
stage: 3
feature: Architecture
---

# RFC: Notify Protocol: Unix Datagram

## 1. Summary

Implement a Unix Datagram socket server that mimics `systemd`'s notification socket.

## 2. Motivation

We need a standard way for apps to signal readiness. `sd_notify` is the standard.

## 3. Detailed Design

Create a socket. Inject `NOTIFY_SOCKET` env var. Listen for `READY=1`.

### Terminology

- **sd_notify**: systemd notification protocol.

### User Experience (UX)

Apps that support systemd work automatically.

### Architecture

`NotifyServer` struct.

### Implementation Details

`tokio::net::UnixDatagram`.

## 4. Drawbacks

- Unix only.

## 5. Alternatives

- HTTP endpoint.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
