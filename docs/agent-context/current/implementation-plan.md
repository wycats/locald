# Phase 10 Implementation Plan: Multi-Service Dependencies

## Goal
Support complex project structures where services depend on each other. Ensure services start in the correct order based on a dependency graph.

## User Requirements
- **App Builder**: "My API crashes if the database isn't running. I want to say `depends_on = ['db']`."
- **Power User**: "I have a complex microservices setup. I need a DAG startup."
- **Contributor**: "I want to implement topological sorting in the process manager."

## Strategy
1.  **Config Schema**: Add `depends_on: Vec<String>` to `ServiceConfig` in `locald-core`.
2.  **Dependency Graph**: In `locald-server`, when starting a project:
    *   Build a graph of services.
    *   Detect cycles (A -> B -> A) and error out.
    *   Perform a topological sort to determine startup order.
3.  **Sequential Startup**: Spawn processes in the sorted order.
    *   *MVP*: Just wait for the `spawn` call to succeed before moving to the next.
    *   *Future*: Wait for port binding or health check.

## Step-by-Step Plan

### Step 1: Schema Update
- [ ] Update `locald-core/src/config.rs` to include `depends_on` in `ServiceConfig`.
- [ ] Update `locald-cli/src/init.rs` to optionally prompt for dependencies (or just leave it manual for now).

### Step 2: Topological Sort Logic
- [ ] Add `petgraph` or implement a simple Kahn's algorithm in `locald-server`.
- [ ] Implement `resolve_startup_order(config: &LocaldConfig) -> Result<Vec<String>>`.

### Step 3: Process Manager Update
- [ ] Refactor `ProcessManager::start` to use the resolved order.
- [ ] Ensure that if a dependency fails to start, dependent services are skipped.

### Step 4: Verification
- [ ] Create a test project with `db` and `api` where `api` depends on `db`.
- [ ] Verify `db` starts before `api`.
- [ ] Verify cycle detection (A -> B -> A) returns an error.
