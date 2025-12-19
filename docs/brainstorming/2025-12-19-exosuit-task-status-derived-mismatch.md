# Note to Exosuit maintainers: task completion mismatch (`exo task list` vs `exo phase status`)

## Summary

In Phase 110 of `dotlocal`, we observed a confusing mismatch where:

- `exo task list` showed tasks `110.1/110.2/110.3` as **completed**
- `exo phase status --format json` showed the same tasks as **pending**

The mismatch was resolved by marking the **implementation-plan steps** as `completed` using `exo impl update-status … completed`.

The core issue is that `exo phase status` appears to **derive task status from implementation-plan step status**, while `exo task list` displays an independently-stored task status. When these drift, users get contradictory answers from core “what’s the status?” commands.

## Why this matters

- Users rely on `exo phase status` as an authoritative “where am I / can I ship?” indicator.
- Users also rely on `exo task list` as the checklist they are explicitly completing.
- When the two disagree, it’s unclear what the system considers “done,” and the “fix” is non-obvious unless you inspect `derived_reason` in JSON output.

This is especially likely to happen when a phase has tasks marked complete early, but later new steps are added that also satisfy the same tasks (or when steps exist but their status is never updated).

## Observed behavior

### `exo task list`

Shows tasks as `completed`:

| ID    | Label                                 | Status    |
| ----- | ------------------------------------- | --------- |
| 110.1 | Define Surface Contract (verbs/nouns) | completed |
| 110.2 | Define stability labels + governance  | completed |
| 110.3 | Update docs to match contract         | completed |

### `exo phase status --format json`

Initially showed tasks as `pending` with a `derived_reason` like:

- “Derived from implementation-plan links: change '<step>' is 'pending' and satisfies '110.1' …”

After running:

- `exo impl update-status "<step>" completed`

…the same JSON output reported the tasks as `completed`.

## Minimal reproduction sketch

This is the smallest conceptual repro we hit (not a copy/paste repo repro):

1. Start a phase with tasks `T1..Tn`.
2. Mark tasks complete (via whatever mechanism `exo task list` represents).
3. Add or keep implementation-plan steps that satisfy those tasks, but leave step status as `pending`.
4. Run:
   - `exo task list` → shows tasks completed
   - `exo phase status --format json` → shows tasks pending (derived from step statuses)

## Diagnosis (what seems to be happening)

- `exo task list` appears to read task status from the task list snapshot/state (explicit status).
- `exo phase status` appears to compute a “derived task status” from the implementation-plan graph:
  - steps/changes → `satisfies = ["110.1", …]`
  - task status = aggregate over those steps’ statuses

This is a sensible model, but it needs to be surfaced clearly because it can contradict the task list.

## Recommendations

### 1) Make `exo phase status` show _both_ statuses explicitly

When there is a discrepancy, show:

- task-list status: `completed`
- derived/plan status: `pending`

and a clear top-level warning like:

> Task status mismatch: task list says completed, but plan-derived status is pending. Run `exo impl update-status …` or update step/task links.

This could be done in both `human` and `json` formats.

### 2) Consider making one source of truth

If the intended truth is plan-derived status, consider:

- removing/soft-deprecating manual task statuses, OR
- automatically syncing task list status from derived status, OR
- making `exo task list` display the derived status (or a derived column) by default.

Right now the system _looks_ like it has two authoritative task status stores.

### 3) Improve affordances for completing steps

Users naturally “complete tasks,” not “complete plan steps.” If step completion is what gates `phase status`, help users get there:

- Add a hint to `exo task list` (or a new command) that lists “steps satisfying this task that are not completed.”
- Provide a convenience command like:
  - `exo task complete 110.1` → marks all steps satisfying `110.1` complete (or prompts).

### 4) Tighten command help / docs

Document in `exo phase status` help/docs that:

- task completion is derived from implementation-plan step status
- `derived_reason` exists (and how to interpret it)
- the correct remediation is to update step status (`exo impl update-status`) rather than changing task list status

## Workaround we used

We resolved the mismatch by marking the relevant plan steps complete:

- `exo impl update-status "Define Surface Contract v1" completed`
- `exo impl update-status "Define stability labels + governance" completed`
- `exo impl update-status "Update docs to match contract" completed`
- `exo impl update-status "Align run/exec semantics + Runner noun" completed`
- `exo impl update-status "Remove Python from CI watchdog" completed`

After that, `exo phase status --format json` reported tasks as `completed` and the repo state was consistent.
