---
title: "Extensibility & Plugins"
stage: 0
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
