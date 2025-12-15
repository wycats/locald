---
title: "Architecture: Daemon + CLI"
stage: 3
feature: Architecture
---

# RFC: Architecture: Daemon + CLI

## 1. Summary

The system shall be split into two logical components: a background daemon (`locald-server`) and a command-line interface (`locald-cli`).

## 2. Motivation

Processes need to run in the background, independent of the terminal session. A daemon ensures that services keep running even if the user closes their terminal. The CLI provides a convenient way to interact with the daemon.

## 3. Detailed Design

The daemon manages the lifecycle of services, logs, and state. The CLI sends commands to the daemon via IPC.

### Terminology

- **Daemon**: The background process (`locald-server`).
- **CLI**: The user-facing command-line tool (`locald-cli`).

### User Experience (UX)

Users run `locald start` to start services. The CLI communicates with the daemon to execute the command.

### Architecture

- `locald-server`: Long-running process.
- `locald-cli`: Ephemeral process.

### Implementation Details

Initially implemented as two separate binaries. Later merged into a single binary (RFC 0025).

## 4. Drawbacks

- More complex architecture than a simple process runner.
- Requires IPC.

## 5. Alternatives

- Run everything in the foreground (like `foreman`).

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- Single binary distribution (RFC 0025).
