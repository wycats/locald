---
title: "Ephemeral Runtime, Persistent Context"
stage: 3
feature: Architecture
---

# RFC: Ephemeral Runtime, Persistent Context

## 1. Summary

Explicitly decouple runtime state (PID) from contextual state (logs, history).

## 2. Motivation

Users need to debug crashes. If the process dies and we lose the logs, we failed.

## 3. Detailed Design

Store logs and history on disk. Keep them even if the process is gone.

### Terminology

- **Ephemeral**: Lasts only as long as the process.
- **Persistent**: Lasts across restarts.

### User Experience (UX)

"Why did it crash?" -> Look at the logs (they are still there).

### Architecture

`LogManager`.

### Implementation Details

File-based logging.

## 4. Drawbacks

- Disk usage.

## 5. Alternatives

- In-memory ring buffer (lost on crash).

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- Log rotation.
