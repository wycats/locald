---
title: "Engineering Excellence"
stage: 0
feature: General
---

# Design: Engineering Excellence

## Error Handling Strategy

- **Goal**: Provide human-readable, actionable error messages while preserving low-level context for debugging.
- **Mechanism**:
  - **Human-Readable Errors**: Use a dedicated enum or trait for errors that map to user-facing steps (e.g., "Failed to create directory" vs. "Permission denied").
  - **Context**: Add context to errors at the point of failure (e.g., "I was trying to mkdir recursive and failed to get permissions on this file").
  - **Logging**: Always log full error details (including backtraces) to a temporary file (e.g., `/tmp/locald.log`).
  - **Verbosity Flags**:
    - Default: Print human-readable error or "Something went wrong. Re-run with --show-error".
    - `--show-error`: Print the last error.
    - `--show-error <code>`: Print the specific error.
    - `--verbose-errors`: Print full error details inline.

## Testing Strategy

- **Goal**: Ensure CLI commands and shell interactions are tested.
- **Mechanism**: Use `trycmd` for markdown-based CLI testing. Use `assert_cmd` for integration tests.
- **Documentation**: Structure the book so that code samples are tested (e.g. using `mdbook-cmdrun` or similar, or just `trycmd` on the docs).

## Implementation Plan (Stage 2)

- [ ] Refactor error handling.
- [ ] Add `trycmd` tests.
- [ ] Verify documentation code samples.

## Context Updates (Stage 3)

List the changes required to `docs/agent-context/` to reflect this feature as "current reality".

- [ ] Create `docs/agent-context/architecture/error-handling.md`
- [ ] Update `docs/agent-context/plan-outline.md` to mark Phase 31 as complete.
