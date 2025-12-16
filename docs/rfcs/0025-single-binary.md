---
title: "Single Binary Distribution"
stage: 3
feature: Architecture
---

# RFC: Single Binary Distribution

## 1. Summary

Merge `locald-server` and `locald-cli` into a single `locald` binary.

## 2. Motivation

Distributing two binaries is annoying. A single binary simplifies updates and versioning.

## 3. Detailed Design

The `locald` binary contains both client and server logic. `locald server start` invokes the server mode.

### Terminology

- **Single Binary**: One executable file.

### User Experience (UX)

Download one file. Run it.

### Architecture

Merge crates or use workspace dependencies.

### Implementation Details

`clap` subcommands.

## 4. Drawbacks

- Larger binary size (marginally).

## 5. Alternatives

- Separate binaries.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

None.
