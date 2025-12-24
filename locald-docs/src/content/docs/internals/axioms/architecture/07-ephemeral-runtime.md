---
title: "Axiom 7: Ephemeral Runtime, Persistent Context"
---


## The Principle

**The runtime state of a service is ephemeral, but the context surrounding it is persistent.**

`locald` treats running processes as transient entities. They can crash, be restarted, or be stopped at any time. However, the _context_ of that service—its logs, its configuration history, its environment variables, and its relationship to other services—must be preserved across these restarts.

## Why It Matters

1.  **Debugging Continuity**: When a service crashes, the developer needs to see the logs _leading up to_ the crash, even if the process is gone.
2.  **Stateful Development**: Developers build mental models of their system over time. The tool should not "forget" what happened just because a process stopped.
3.  **Resilience**: The system assumes failure is normal. By decoupling the process lifecycle from the data lifecycle, `locald` remains robust even when user code is unstable.

## Implementation

- **Log Retention**: Logs are stored on disk and are accessible even when the service is down.
- **Status History**: The dashboard displays not just the current state, but recent exit codes and restart counts.
- **Configuration Stability**: Changes to configuration are applied gracefully, preserving the identity of the service even as its definition evolves.

