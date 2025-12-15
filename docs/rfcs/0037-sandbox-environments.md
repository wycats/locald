---
title: "Sandbox Environments: Explicit Isolation"
stage: 3
feature: Architecture
---

# RFC: Sandbox Environments: Explicit Isolation

## 1. Summary

Implement `--sandbox <NAME>` to isolate environments.

## 2. Motivation

Testing `locald` shouldn't break the user's real setup.

## 3. Detailed Design

Isolate `XDG_*` dirs. Panic if `LOCALD_SOCKET` is set without sandbox.

### Terminology

- **Sandbox**: An isolated environment.

### User Experience (UX)

Safe testing.

### Architecture

Startup logic.

### Implementation Details

Env var manipulation.

## 4. Drawbacks

- Complexity in tests.

## 5. Alternatives

- Docker (slow).

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
