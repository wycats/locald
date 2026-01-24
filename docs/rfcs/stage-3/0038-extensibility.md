---
title: "Extensibility & Plugins"
stage: 3
feature: General
---

# Design: Extensibility & Plugins

## Plugin Mechanism

- **Goal**: Allow users to extend `locald` with custom services and package them for distribution.
- **Mechanism**:
  - Formal plugin system for custom services (e.g., `locald.localhost`).
  - Support for packaging customizations (e.g., `locald package /path/to/customizations`) into a distributable format.
  - Allow distribution of "flavored" `locald` binaries or configuration bundles without requiring recompilation.

## Implementation Plan (Stage 2)

- [ ] Define plugin interface.
- [ ] Implement plugin loader.
- [ ] Create example plugin.

## Context Updates (Stage 3)

List the changes required to `docs/agent-context/` to reflect this feature as "current reality".

- [ ] Create `docs/agent-context/features/extensibility.md`
- [ ] Update `docs/agent-context/plan-outline.md` to mark Phase 29 as complete.

## Implementation Status (Stage 3)

**Promoted to Stage 3**: The plugin system is fully implemented (~4,800 lines).

### Implemented Components

- **Plugin Runner** (`locald-server/src/plugins/runner.rs`): Wasmtime engine integration, WASM component execution
- **Plan System** (`locald-server/src/plugins/plan.rs`): Service topology, dependency resolution, execution ordering
- **Plugin Orchestration** (`locald-server/src/plugins/mod.rs`): Lifecycle management, state machine coordination
- **CLI Commands** (`locald-cli/src/plugin.rs`): `locald plugin build`, `locald plugin list`, `locald plugin run`
- **WIT Interface** (`locald-server/wit/locald-plugin.wit`): Formal contract between host and plugins

### API Stability Note

The plugin system is feature-flagged as `experimental-plugins`. This reflects an **API stability policy**, not implementation quality. The WIT interface may evolve as we validate the design with real-world plugins.

### Example Plugins

- **Redis Plugin** (`examples/redis-plugin/`): Full working example with E2E test coverage

### Original Checklist (Completed)

- [x] Define plugin interface (WIT)
- [x] Implement plugin loader (Wasmtime runner)
- [x] Create example plugin (redis-plugin)
