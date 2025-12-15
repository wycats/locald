---
title: "Privileged Ports: Capabilities over Root"
stage: 3
feature: Architecture
---

# RFC: Privileged Ports: Capabilities over Root

## 1. Summary

The daemon shall run as the user, but use `setcap` to bind to privileged ports (80/443).

## 2. Motivation

We want to bind port 80 for clean URLs, but running the entire daemon as root violates the principle of least privilege and Axiom 04 (Process Ownership).

## 3. Detailed Design

The binary is granted `cap_net_bind_service` capability. This allows it to bind to ports < 1024 without being root.

### Terminology

- **Capabilities**: Linux capabilities (fine-grained privileges).
- **cap_net_bind_service**: The capability to bind to privileged ports.

### User Experience (UX)

Users run `locald admin setup` (which uses sudo) once to apply the capabilities.

### Architecture

N/A

### Implementation Details

`setcap cap_net_bind_service=+ep path/to/binary`.

## 4. Drawbacks

- Requires `sudo` for setup.
- Capabilities are cleared when the binary is recompiled/replaced.

## 5. Alternatives

- Run as root (insecure).
- Port forwarding (iptables).
- `authbind`.

## 6. Unresolved Questions

None.

## 7. Future Possibilities

- `locald-shim` (Phase 21) to avoid giving capabilities to the main binary.
