---
title: "CLI Surface Overhaul & Dashboard Integration"
stage: 0 # 0: Strawman
feature: CLI Redesign
---

# RFC: CLI Surface Overhaul & Dashboard Integration

## 1. Summary

This RFC proposes a comprehensive overhaul of the `locald` CLI surface area to align with the project's core axioms (Zero-Friction Start, Daemon-First) and improve conceptual integrity. It reorganizes commands around **User Intent**, resolves semantic ambiguities (e.g., `try` vs `run`), and formalizes the relationship between the CLI, the Registry, and the Dashboard.

## 2. Motivation

The current CLI has accumulated commands organically, leading to minor inconsistencies and "orphaned" workflows.

- **Ambiguity**: `locald run` and `locald try` are semantically similar but functionally distinct.
- **Friction**: `locald up` fails confusingly outside a workspace, violating the "Zero-Friction" principle for users who just want to start the daemon.
- **Disconnect**: The Registry supports workspace concepts, but the CLI lacks a direct bridge to the Dashboard.
- **Cognitive Load**: Manual "pinning" in the registry is high-friction.

We aim to restructure the CLI so that it intuitively maps to the user's mental model of "Activation", "Development", and "Platform Management".

## 3. Detailed Design

### 3.1. Conceptual Command Structure

We categorize commands by **User Intent**:

#### 1. Workspace Activation (Start Here)

_Commands that establish context and bring the environment to life._

- `locald init`: Scaffold a new `locald.toml`.
- `locald up [path]`: The "Do It" command. Starts daemon, registers project (implicit "Recent"), boots services. **Context-aware**: If no config exists, it just ensures the daemon is up.
- `locald dashboard`: **(New)** Opens the visual workspace in the default browser.

#### 2. The Development Loop (Daily Work)

_Commands used frequently while coding._

- `locald status`: High-level view.
- `locald logs [service]`: Stream output.
- `locald exec <service> <cmd>`: **(Renamed from `run`)** Run a task inside a service's context (e.g., `rails db:migrate`). Matches `docker exec`.
- `locald try <cmd>`: Experimentation mode. Ephemeral execution.
- `locald restart <service>`: Bounce a service.
- `locald stop [service]`: Pause a service.

#### 3. Configuration (Defining Architecture)

_Commands that modify `locald.toml`._

- `locald add <cmd>`: Quick-add shell service.
- `locald service add <type>`: Add complex services.
- `locald service reset <name>`: Wipe data and restart.
- `locald config show`: View resolved config.

#### 4. Platform Management (The Machinery)

_Commands that manage the `locald` installation._

- `locald server <start|shutdown>`: Control background daemon.
- `locald monitor`: TUI.
- `locald trust`: Install Root CA.
- `locald admin`: Privileged operations.
- `locald registry <list|clean>`: Manage known projects.

### 3.2. Key Changes

#### Rename `run` to `exec`

- **Old**: `locald run web rails c`
- **New**: `locald exec web rails c`
- **Rationale**: `exec` is the industry standard (Docker, K8s) for "execute inside an existing context". `run` is too generic and conflicts with `try`.

#### Context-Aware `up`

- **Behavior**:
  - If `locald.toml` exists (or path provided): Start daemon, register project, start services.
  - If NO config: Start daemon only. Print "Daemon is running. No locald.toml found."
- **Rationale**: Supports "bring up locald" use case without erroring.

#### Registry Philosophy: Implicit vs. Explicit

- **Recent (Implicit)**: `locald up` automatically updates the project's `last_seen` timestamp in the registry. These appear in "Recent Projects" in the Dashboard.
- **Pinned (Explicit)**: User manually pins via Dashboard or CLI. These are "Favorites".
- **Change**: De-emphasize manual `pin` in CLI help.

### 3.3. Dashboard Integration

- `locald dashboard`: Opens `http://locald.localhost`.
- The Dashboard will serve as the primary interface for "Pinning/Unpinning" projects, reducing CLI surface area for these tasks.

## 4. Implementation Plan (Stage 2)

- [ ] **Phase 1: Rename & Alias**
  - [ ] Rename `Run` variant to `Exec` in `locald-cli`.
  - [ ] Add `run` as a hidden alias for backward compatibility (with deprecation warning).
- [ ] **Phase 2: Context-Aware Up**
  - [ ] Modify `locald up` handler to check for `locald.toml` before attempting registration.
- [ ] **Phase 3: Dashboard Command**
  - [ ] Implement `locald dashboard` to open browser.
- [ ] **Phase 4: Registry Refinement**
  - [ ] Update `locald up` to trigger "touch" logic in Registry (if not already present).
  - [ ] Update help text to de-emphasize `pin`.

## 5. Context Updates (Stage 3)

- [ ] Update `docs/manual/getting-started.md` to use `exec`.
- [ ] Update `docs/manual/reference/cli.md`.
