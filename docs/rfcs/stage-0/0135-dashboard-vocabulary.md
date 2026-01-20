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
- Enable “pin and monitor” workflows for core services.

## Non-Goals

- A full redesign of the dashboard UI.
- New dashboard features beyond naming/terminology.

## Proposed Vocabulary (Draft)

- **Workspace**: A locald project directory with a `locald.toml`.
- **Service**: A running unit (exec/container/worker/site).
- **Monitor**: View logs + health + status in real time.
- **Pin**: Persist a workspace or service in the dashboard quick list.
- **Cleanup**: Remove stopped services, cache, or temp resources.
- **Stop**: Gracefully terminate services (preserve config/state).
- **Remove**: Delete runtime artifacts (logs, temporary volumes, caches).

## UX Expectations

- “Pin” always means “persist in UI list.”
- “Monitor” is the action for the log/health view (CLI: `locald monitor`).
- “Cleanup” never implies deleting source files; only runtime artifacts.

## Open Questions

1. Should “monitor” be the CLI command or a synonym for “logs”?
2. Should “cleanup” be a global action or per-workspace?
3. How should pinned items sync between CLI and dashboard?

## References

- `locald monitor` (TUI)
- CLI help text and dashboard labels
