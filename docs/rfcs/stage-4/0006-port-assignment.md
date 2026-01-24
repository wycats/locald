---
title: "Port Assignment: Dynamic & Env Var"
stage: 3
feature: Architecture
---

# RFC: Port Assignment: Dynamic & Env Var

## 1. Summary

The daemon shall dynamically assign free ports to services and inject them as the `PORT` environment variable.

## 2. Motivation

Hardcoding ports leads to conflicts when running multiple projects or services. Dynamic assignment ensures that services can always start.

## 3. Detailed Design

When starting a service, the daemon finds a free port on the loopback interface. It sets the `PORT` environment variable for the child process. Services must be written to respect this variable.

### Terminology

- **Dynamic Port**: A port assigned at runtime.
- **PORT**: The environment variable.

### User Experience (UX)

Users do not need to manage ports manually. They can access services via the proxy or by checking the assigned port.

### Architecture

The `ProcessManager` handles port allocation.

### Implementation Details

Bind to port 0 to let the OS assign a port, then retrieve it.

## 4. Drawbacks

- Services must support the `PORT` env var.

## 5. Alternatives

- Static port configuration in `locald.toml`.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- Port discovery for services that don't support `PORT` (Phase 23).
