---
title: Dashboard Vocabulary and User Mental Model
stage: 0
feature: Dashboard
---

# RFC 0135: Dashboard Vocabulary and User Mental Model ✅ RESOLVED

**Stage**: 0 (Idea) → **Vocabulary Locked**
**Author**: locald team
**Created**: 2026-01-20
**Resolved**: 2026-01-21

## Summary

Define a consistent vocabulary for dashboard actions and states (e.g., "pin," "monitor," "cleanup"), shared across CLI, docs, and UI.

## Motivation

The dashboard is central to user trust. Today, labels and actions are inconsistent across surfaces. A shared vocabulary reduces confusion and supports announcement messaging.

## Goals

- Establish canonical terms for common actions and states.
- Align CLI help, docs, and UI labels.
- Define the distinction between "keep running", "show logs", and "hide/stop".

## Non-Goals

- A full redesign of the dashboard UI.
- New dashboard features beyond naming/terminology.

## Canonical Vocabulary ✅ LOCKED

> **Resolution**: These definitions are authoritative. All CLI help text, docs, and UI labels must use these terms consistently.

### Core Concepts

| Term          | Definition                                                                     | Surface      |
| ------------- | ------------------------------------------------------------------------------ | ------------ |
| **Workspace** | A locald project directory with a \`locald.toml\`                               | All          |
| **Service**   | A running unit (exec/container/worker/site) within a workspace                 | All          |

### Runtime Policy (Registry Layer)

| Term         | Definition                                                                     | Command                |
| ------------ | ------------------------------------------------------------------------------ | ---------------------- |
| **Enabled**  | Present in config; starts automatically on daemon boot (default)               | Implicit default       |
| **Disabled** | Present in config; does NOT start automatically on daemon boot                 | \`locald registry disable\` |
| **Pin**      | Registry policy: retain project + autostart on daemon boot                     | \`locald registry pin\`  |
| **Unpin**    | Remove from registry; eligible for GC                                          | \`locald registry unpin\`|

> **Key insight**: "Pin" is **only** for registry policy (retain + autostart). The dashboard does NOT use "pin" — it uses "monitor" instead.

### Dashboard Actions (UI Layer)

| Term         | Definition                                                                     | Icon           |
| ------------ | ------------------------------------------------------------------------------ | -------------- |
| **Monitor**  | Focus the log/status panel on a selected service (dashboard action)            | 👁️ (eye/monitor)|
| **Favorite** | Persist a service in the dashboard quick list (UI-only; no runtime effect)     | ⭐ (star)       |

> **⚠️ Dashboard must NOT use "pin"** — this was previously confusing. Use "monitor" for focus actions.

### Lifecycle Actions

| Term        | Definition                                                                     | Command                |
| ----------- | ------------------------------------------------------------------------------ | ---------------------- |
| **Cleanup** | Remove stopped services, cache, or temp resources (never source files)         | \`locald registry clean\`|
| **Stop**    | Gracefully terminate services (preserve config/state)                          | \`locald stop\`          |
| **Remove**  | Delete runtime artifacts (logs, temporary volumes, caches)                     | \`locald service reset\` |

## UX Expectations ✅ LOCKED

- **"Pin"** is reserved for registry policy (retain + autostart). The dashboard must NOT use this term.
- **"Monitor"** is the dashboard action for focusing the log/health view.
- **"Favorite"** means "persist in UI list" (UI only; no runtime effect).
- **"Cleanup"** never implies deleting source files; only runtime artifacts.

## Service State Model ✅ RESOLVED

### Two-Persona Model

| Mode               | Default Behavior                                | User Action to Change        |
| ------------------ | ----------------------------------------------- | ---------------------------- |
| **On-by-default**  | Services **enabled** unless explicitly disabled | \`locald registry disable\`    |
| **Off-by-default** | Services **disabled** unless explicitly pinned  | \`locald registry pin\`        |

**Preferred mode**: On-by-default (most common user expectation).

**Off-by-default** is available for power users who prefer manual control over what starts on boot.

### Disabled Service UX

If a service is **disabled**, its domain should still resolve. Requests should return a dedicated page that:

- says the service is disabled,
- offers an "Enable" action,
- and then transitions directly into the existing build/start log UI.

## Open Questions ✅ ALL RESOLVED

1. **Should the CLI expose "disable/enable" explicitly?**
   → **Yes.** \`locald registry disable\` and \`locald registry enable\` (or equivalently, \`pin\`).

2. **Should "cleanup" be a global action or per-workspace?**
   → **Both.** \`locald registry clean\` is global; \`locald service reset <name>\` is per-service.

3. **How should "enabled/disabled" policy sync between CLI and dashboard?**
   → **Registry is source of truth.** Dashboard reads/writes to the same registry state that the CLI uses.

## References

- RFC 0116: MAP Scope (vocabulary section)
- RFC 0112: User Programming Model Audit (Conflict Cards C-007, C-008)
- \`locald monitor\` (TUI)
- CLI help text and dashboard labels
