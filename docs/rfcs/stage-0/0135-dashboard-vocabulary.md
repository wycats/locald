---
title: Dashboard Vocabulary and User Mental Model
stage: 0
feature: Dashboard
---

# RFC 0135: Dashboard Vocabulary and User Mental Model

**Stage**: 0 (Idea)
**Author**: locald team
**Created**: 2026-01-20

## Summary

Define a consistent vocabulary for dashboard actions and states (e.g., “pin,” “monitor,” “cleanup”), shared across CLI, docs, and UI.

## Motivation

The dashboard is central to user trust. Today, labels and actions are inconsistent across surfaces. A shared vocabulary reduces confusion and supports announcement messaging.

## Goals

- Establish canonical terms for common actions and states.
- Align CLI help, docs, and UI labels.
- Define the distinction between “keep running”, “show logs”, and “hide/stop”.

## Non-Goals

- A full redesign of the dashboard UI.
- New dashboard features beyond naming/terminology.

## Proposed Vocabulary (Draft)

- **Workspace**: A locald project directory with a `locald.toml`.
- **Service**: A running unit (exec/container/worker/site).
- **Monitor**: Dashboard action that focuses the right-hand log/status panel on a selected service.
- **Enabled**: Present in configuration and starts automatically (default mode).
- **Disabled**: Present in configuration but does not start automatically.
- **Pin**: “Keep it running” (the opposite of disabled). In the default mode, pinning is redundant with “enabled” and may be unnecessary.
- **Favorite**: Persist a workspace or service in the dashboard quick list (UI-only; does not affect runtime).
- **Cleanup**: Remove stopped services, cache, or temp resources.
- **Stop**: Gracefully terminate services (preserve config/state).
- **Remove**: Delete runtime artifacts (logs, temporary volumes, caches).

## UX Expectations

- “Pin” always means “keep it running” (runtime policy), and is the opposite of “disabled”.
- “Favorite” always means “persist in UI list” (UI only).
- “Monitor” is the dashboard action for the focused log/health view.
- “Cleanup” never implies deleting source files; only runtime artifacts.

## Service State Model (Draft)

We need to support both of these modes, but the vocabulary must remain consistent:

1. **On-by-default** (preferred): services are enabled unless explicitly disabled.
2. **Off-by-default** (optional): everything starts disabled; users “pin” workspaces/services to keep them running.

If a service is **disabled**, its domain should still resolve. Requests should return a dedicated page that:

- says the service is disabled,
- offers an “Enable” action,
- and then transitions directly into the existing build/start log UI.

## Open Questions

1. Should the CLI expose “disable/enable” explicitly (vs only in the dashboard)?
2. Should “cleanup” be a global action or per-workspace?
3. How should “enabled/disabled” policy sync between CLI and dashboard?

## References

- `locald monitor` (TUI)
- CLI help text and dashboard labels
