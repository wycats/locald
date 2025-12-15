---
agent: agent
description: Rescues the current phase in a new chat session after a previous session was abandoned or corrupted.
---

### Phase Rescue & Restoration

Use this prompt when starting a new chat to continue a phase, especially if the previous session was confused, hallucinating, or crashed.

**Goal**: Re-establish truth by reconciling the **Context** (what we *thought* we did) with **Reality** (what is actually in the code).

#### 1. Restore Context
- Run \`exo context restore\`.
- Read the output carefully. This is the "Canon".

#### 2. Reality Check (The Audit)
- **Do not assume the Context is perfect.** The previous session may have failed to update it.
- Look at the `task-list.toml`. For the last completed task and the current pending task:
  - **Verify in Code**: specificially check the files to see if the code changes are actually present.
  - **Verify in Tests**: Check if the tests for those features exist and pass.

#### 3. Reconcile
- **If Code exists but Task is Pending**: Update `task-list.toml` to mark it as done (and update `walkthrough.toml` if needed).
- **If Task is Done but Code is missing**: This is a critical failure. Mark the task as `pending` in `task-list.toml` and note the regression.
- **If `walkthrough.toml` is empty/stale**: Summarize the work that *has* been done so far based on your code audit.

#### 4. Resume
- Once Context and Reality are aligned, identify the next true objective.
- Continue execution.
