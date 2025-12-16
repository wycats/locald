# Research: Docker Integration

## Goal

Determine the best way to interact with the Docker Daemon from `locald-server`.

## Options

### 1. `bollard` Crate

- **Pros**:
  - Pure Rust, async, uses `tokio`.
  - Type-safe API for Docker.
  - Handles streaming logs, events, etc.
- **Cons**:
  - Heavy dependency? (Need to check).
  - Might require OpenSSL (portability issues?).

### 2. `docker` CLI Wrapper

- **Pros**:
  - Zero dependencies.
  - "Interface Parity" (if the user has docker installed, it works).
- **Cons**:
  - Parsing stdout/stderr is brittle.
  - Handling signals and lifecycle is harder.
  - No direct access to events stream (have to run `docker events`).

## Decision Matrix

| Feature          | Bollard | CLI Wrapper             |
| :--------------- | :------ | :---------------------- |
| **Reliability**  | High    | Medium                  |
| **Complexity**   | Medium  | Low (initially) -> High |
| **Dependencies** | High    | Low                     |
| **Async**        | Native  | Manual (Tokio Process)  |

## Recommendation

Start with **Bollard**. `locald` is a daemon; it should be robust. Parsing CLI output is a recipe for pain in a long-running process.

## Implementation Details

- We need to connect to the local Docker socket (`/var/run/docker.sock`).
- We need to handle image pulling (streaming progress?).
- We need to map ports.

## Open Questions

- Does `bollard` support `podman`? (Usually yes, if socket is compatible).
