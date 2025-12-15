# Phase 26 Implementation Plan: Configuration & Constellations

**RFC**: [docs/rfcs/0026-configuration-hierarchy.md](../../rfcs/0026-configuration-hierarchy.md)

## Goal

Implement a structured, cascading configuration system with "Crisp" merging rules, provenance tracking, and a persistent registry for "Always Up" services.

## 1. Global Configuration & Provenance

- **Objective**: Establish the base layer of configuration and the tooling to inspect it.
- **Tasks**:
  - Define `GlobalConfig` struct (mapped to `~/.config/locald/config.toml`).
  - Implement loading logic for `GlobalConfig`.
  - Create `ConfigLoader` that tracks the source of each value (Provenance).
  - Implement `locald config show --provenance` CLI command.

## 2. The Registry (Persistence)

- **Objective**: Track known projects and their state across daemon restarts.
- **Tasks**:
  - Define `Registry` struct (mapped to `~/.local/share/locald/registry.json`).
  - Implement `Registry::load()` and `Registry::save()`.
  - Implement CLI commands:
    - `locald registry list`
    - `locald pin <path>`
    - `locald unpin <path>`
    - `locald registry clean`
  - Integrate Registry loading into Daemon startup.

## 3. Workspace Discovery & Merging

- **Objective**: Implement the hierarchy (Global -> Context -> Workspace -> Project) and the merging logic.
- **Tasks**:
  - Implement `Workspace` discovery (Git root or `locald.workspace.toml`).
  - Implement `Context` discovery (recursive walk up).
  - Implement the "Crisp" merging logic:
    - `[env]`: Merge (Child overrides Parent).
    - `[secrets]`: Overlay (Runtime resolution).
    - `[resources]`: Attach (Injection).
    - `depends_on`: Replace.
  - Implement Variable Interpolation (`${VAR}`, `${project.root}`, etc.).

## 4. Integration

- **Objective**: Wire everything into the Daemon and ServiceConfig.
- **Tasks**:
  - Update `ServiceConfig` loading to use the new `ConfigLoader`.
  - Ensure `locald up` respects the merged configuration.
  - Verify "Always Up" services start automatically if pinned.
