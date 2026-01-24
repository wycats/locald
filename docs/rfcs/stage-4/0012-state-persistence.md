---
title: "State Persistence: JSON in XDG Data Dir"
stage: 3
feature: Architecture
---

# RFC: State Persistence: JSON in XDG Data Dir

## 1. Summary

The daemon shall persist its state (running services, PIDs) to a JSON file in the XDG data directory.

## 2. Motivation

The daemon needs to remember running services across restarts (e.g., for upgrades or crash recovery). In-memory state is lost on exit.

## 3. Detailed Design

Store state in `~/.local/share/locald/state.json`.

### Terminology

- **XDG Data Dir**: Standard location for user data (`$XDG_DATA_HOME` or `~/.local/share`).

### User Experience (UX)

Transparent. Services "remember" they were running.

### Architecture

`StateManager` handles loading and saving.

### Implementation Details

Use `serde_json`.

## 4. Drawbacks

- Disk I/O.
- Potential for corruption (need atomic writes).

## 5. Alternatives

- SQLite (overkill).
- `sled` (embedded DB).

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- Database for more complex state (history).
