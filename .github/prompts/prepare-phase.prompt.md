---
agent: agent
description: Prepares the Implementation Plan for the *next* phase after the current phase is finished.
---

### Phase Staging

Use this prompt **after** `phase-transition` is complete, but **before** starting the new phase in a new chat.

**Goal**: Set the stage for the next phase so the next agent can hit the ground running.

#### 1. Identify Next Phase
- Read `docs/agent-context/plan.toml`.
- Identify the next phase in the sequence.

#### 2. Draft Implementation Plan
- Create or update `docs/agent-context/current/implementation-plan.toml`.
- **Goal**: Copy the high-level goal from `plan.toml`.
- **Proposed Changes**: Draft a high-level outline of changes based on `docs/agent-context/future/ideas.toml` or known requirements.
- **Verification**: Add a placeholder for verification steps.

#### 3. Clean Up
- Remove any items from `docs/agent-context/future/` that are now covered by this new plan.

#### 4. Handoff
- Do **not** start the phase.
- Do **not** write code.
- Just leave the `implementation-plan.toml` ready for the next session to review and refine.
