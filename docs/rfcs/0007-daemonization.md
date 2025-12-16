---
title: "Daemonization: Self-Managed"
stage: 3
feature: Architecture
---

# RFC: Daemonization: Self-Managed

## 1. Summary

The `locald-server` binary shall handle its own daemonization.

## 2. Motivation

Relying on the CLI or shell to background the process creates complexity around process groups, signal handling, and terminal detachment. Self-daemonization ensures consistent behavior.

## 3. Detailed Design

The binary forks into the background, detaches from the terminal, and manages its own PID file.

### Terminology

- **Daemonize**: The process of becoming a background daemon.

### User Experience (UX)

`locald server start` returns immediately, leaving the daemon running in the background.

### Architecture

Use the `daemonize` crate.

### Implementation Details

A `--foreground` flag is provided for debugging.

## 4. Drawbacks

- Complexity in the binary.

## 5. Alternatives

- Use `systemd` or `launchd` (too heavy for a dev tool).
- Use `nohup` (less control).

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
