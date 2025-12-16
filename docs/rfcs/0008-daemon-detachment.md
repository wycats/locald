---
title: "Daemon Detachment: setsid"
stage: 3
feature: Architecture
---

# RFC: Daemon Detachment: setsid

## 1. Summary

Use `setsid` when spawning `locald-server` to create a new session and fully detach it from the CLI's terminal.

## 2. Motivation

Simply spawning a background process isn't enough; if the CLI is killed (Ctrl-C), the child might die if it's in the same process group. `setsid` ensures the daemon survives the CLI's termination.

## 3. Detailed Design

The CLI uses `setsid` (or equivalent logic) to spawn the server in a new session.

### Terminology

- **setsid**: Create a new session.

### User Experience (UX)

Robustness: The daemon doesn't die when the terminal is closed.

### Architecture

Part of the `locald server start` logic.

### Implementation Details

Use `std::os::unix::process::CommandExt::setsid`.

## 4. Drawbacks

- Unix-specific.

## 5. Alternatives

- Double fork.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
