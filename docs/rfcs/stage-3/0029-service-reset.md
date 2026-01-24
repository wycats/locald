---
title: "Service Reset: Explicit Command"
stage: 3
feature: Architecture
---

# RFC: Service Reset: Explicit Command

## 1. Summary

Implement `locald service reset <name>` to wipe data and restart a service.

## 2. Motivation

Stateful services (Postgres) need to be reset sometimes. Manual deletion of data dirs is error-prone.

## 3. Detailed Design

Stop service -> Delete data dir -> Start service.

### Terminology

- **Reset**: Wipe state and restart.

### User Experience (UX)

`locald service reset db`.

### Architecture

IPC command.

### Implementation Details

`fs::remove_dir_all`.

## 4. Drawbacks

- Data loss (intentional).

## 5. Alternatives

- Manual `rm -rf`.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
