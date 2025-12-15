---
title: "Design: Constellations & Configuration Philosophy"
---

**Goal**: Define how `locald` manages groups of services ("Constellations") and how configuration cascades through the system.

## The Problem

Currently, `locald` treats every project as an isolated island defined by its `locald.toml`. However, users often work with:

1.  **Monorepos**: Multiple projects in one repo that should be managed together.
2.  **Polyrepos**: Distinct repos that logically form a single application (e.g., `frontend` + `backend` + `worker`).
3.  **Directory Contexts**: A `~/Code/work` directory might need different defaults (e.g., different base domain, different env vars) than `~/Code/personal`.
4.  **Root Config / Subdirectory Execution**: A project where the `locald.toml` sits at the root, but the commands need to execute in a subdirectory (e.g., `frontend/` or `packages/app/`).

## The Concept: Constellations

A **Constellation** is a logical grouping of services.

- **Implicit Constellation**: A directory containing multiple projects (e.g., a monorepo root).
- **Explicit Constellation**: A configuration file (e.g., `locald.workspace.toml`?) that explicitly lists projects, potentially across different paths.

## Configuration Philosophy

We need a cascading configuration system that respects the hierarchy:

1.  **Global** (`~/.config/locald/config.toml`): User-wide defaults (theme, root directory, global "always up" list).
2.  **Context** (Directory-based): Configuration applied to a directory tree (e.g., `~/Code/.locald.toml`). _Open Question: How do we discover this without walking up the tree on every command?_
3.  **Constellation** (Workspace): Configuration for a group of projects.
4.  **Project** (`locald.toml`): The specific project configuration.

### "Always Up" & The Registry

To support "Always Up" services, `locald` needs a **Registry** of known projects.

- **Registration**: `locald register .` or implicit on `locald start`.
- **Persistence**: The registry is stored in `~/.local/share/locald/registry.json`.
- **State**: Tracks `path`, `last_seen`, `always_up` flag.
- **Autostart**: On daemon startup, `locald` reads the registry and starts any project marked `always_up`.

### Environment Variables

How do we map this config to environment variables?

- **Injection**: `locald` already injects vars into the process.
- **Cascading**: Context/Constellation config can define `[env]` sections that merge down to the project.
- **Isolation**: Changes to Global/Context config should trigger a restart of affected services (via the Config Watcher).

## UX Implications

- **Dashboard**:
  - Group services by Constellation.
  - Toggle "Always Up" via a checkbox/pin icon.
  - Visual indicator for "Context" (e.g., "Inherited from ~/Code").
- **CLI**:
  - `locald config set --global ...`
  - `locald config set --context ...`
  - `locald service pin <name>` (alias for setting always_up).

## Open Questions

1.  **Discovery**: How do we efficiently find "Context" config?
2.  **Naming**: Is "Constellation" the right term? Or "Workspace"? "Group"?
3.  **Conflicts**: What happens if two projects in a Constellation claim the same port? (Already handled by dynamic assignment, but what about fixed ports?)

## Resource Scoping & Attachment

Resources (like Postgres) should be flexible:

- **Default**: Scoped to the Project + Environment (e.g., `dev`, `test`).
- **Shared**: Defined at the Constellation level and attached to multiple projects (e.g., a shared DB for frontend and backend).
- **Global**: Defined globally (e.g., a shared Redis for all local dev).

## Configuration Rigor

We need to formalize the configuration formats:

- **`locald.toml`**: Project-level config.
- **`locald.workspace.toml`** (or similar): Constellation-level config.
- **Manifest Unification**: Can we use a single schema that adapts based on context?

## Tracking

- **Ad-Hoc Execution**: Tracked in Phase 31.
- **CNB Support**: Tracked in Phase 32.
