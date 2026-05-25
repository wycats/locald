---
agent: agent
description: Prepares the Implementation Plan for the *next* phase after the current phase is finished.
---

### Phase Staging

Use this prompt **after** `phase-transition` is complete, but **before** starting the new phase in a new chat.

**Goal**: Set the stage for the next phase so the next agent can hit the ground running.

#### 1. Identify Next Phase
- Run `exo status` and `exo plan review`.
- Identify the next phase in the sequence.

#### 2. Draft Implementation Plan
- Create or update the current implementation plan artifact.
- **Goal**: Copy the high-level goal from `exo plan review` / `exo phase read-details` output.
- **Proposed Changes**: Draft a high-level outline of changes based on `exo idea list` or known requirements.
- **Verification**: Add a placeholder for verification steps.

#### 3. Clean Up
- Remove any items from `docs/agent-context/future/` that are now covered by this new plan.

#### 4. Handoff
- Do **not** start the phase.
- Do **not** write code.
- Just leave the implementation plan artifact ready for the next session to review and refine.
