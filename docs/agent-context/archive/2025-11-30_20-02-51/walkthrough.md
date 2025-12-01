# Phase 10 Walkthrough: Multi-Service Dependencies

## Overview
In this phase, we are adding support for service dependencies. This allows users to define startup order (e.g., start the database before the API).

## Key Decisions

### 1. Topological Sort
We use a topological sort (Kahn's algorithm) to determine the startup order. This ensures that all dependencies are started before the service that depends on them. We detect circular dependencies and fail if one is found.

### 2. MVP: Start Order Only
For this phase, `depends_on` only guarantees that the *process spawn command* is issued in the correct order. It does not wait for the service to be "ready" (listening on a port), as that requires health checks which are out of scope for now.

## Changes

### Codebase
- **`locald-core`**: Added `depends_on` field to `ServiceConfig`.
- **`locald-cli`**: Updated `init` command to initialize `depends_on`.
- **`locald-server`**: 
    - Implemented `resolve_startup_order` using Kahn's algorithm.
    - Updated `ProcessManager::start` to start services in the resolved order.
    - Added unit tests for dependency resolution and cycle detection.

### Documentation
- Updated `task-list.md` to reflect progress.
