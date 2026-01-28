# Exo Tool Friction Log

This document captures friction points encountered while using the `exo` CLI tool for project management.

## 2026-01-28: Plan State Synchronization

### Issue: No way to mark historical epochs/phases as complete

**Context**: Needed to mark E1 (MVP) and E2 (Refinement) epochs as completed since all their phases were done.

**Friction**:
1. `exo plan update-status E1 completed` failed with "Item not found" - the command doesn't support updating epoch status directly
2. `exo epoch finish` only works for the *active* epoch, not historical ones
3. `exo task complete` only works for tasks in the *active* phase

**Workaround**: Had to edit `docs/agent-context/plan.toml` directly, which violates the AGENTS.md guidance to use exo CLI for plan modifications.

**Suggested Fix**: Add commands like:
- `exo epoch mark-complete <epoch-id>` - Mark a historical epoch as completed
- `exo phase mark-complete <phase-id>` - Mark a historical phase as completed
- Or extend `exo plan update-status` to work with epoch/phase IDs

---

### Issue: CLI derives epoch status from phases, ignoring explicit status field

**Context**: Set `status = "completed"` on E1 and E2 epochs, but `exo epoch list` still showed them as "pending".

**Friction**:
1. The CLI computes epoch status dynamically from phase statuses
2. Phases must use `status = "completed"` (not `"complete"`) to count toward epoch completion
3. The `reviewed = true` flag is also required for proper display

**Workaround**: Had to update all phase statuses from `"complete"` to `"completed"` and add `reviewed = true` to epochs.

**Observation**: The status vocabulary is inconsistent:
- Tasks use `"complete"`
- Phases need `"completed"` for the CLI to recognize them
- Epochs need both `status = "completed"` AND `reviewed = true`

**Suggested Fix**: Standardize on a single status vocabulary across tasks, phases, and epochs.
