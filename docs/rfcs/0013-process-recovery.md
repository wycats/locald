---
title: "Process Recovery: Kill & Restart"
stage: 3
feature: Architecture
---

# RFC: Process Recovery: Kill & Restart

## 1. Summary

When the daemon restarts, it shall kill any "zombie" processes from the previous session and restart them.

## 2. Motivation

Adopting existing processes is complex due to lost I/O pipes (stdout/stderr). It's safer and cleaner to restart them to re-establish log capture and control.

## 3. Detailed Design

On startup, the daemon reads the `state.json`. For each service that was "Running", it checks if the PID exists. If so, it kills it. Then it starts the service anew.

### Terminology

- **Zombie**: A process left running by a previous daemon instance (not a technical zombie process).

### User Experience (UX)

Services restart automatically after a daemon update.

### Architecture

Startup logic in `locald-server`.

### Implementation Details

`nix::sys::signal::kill`.

## 4. Drawbacks

- Service interruption.

## 5. Alternatives

- FD passing (complex).
- `reptyr` (hacky).

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- Zero-downtime restarts (requires FD passing).
