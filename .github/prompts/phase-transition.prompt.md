---
agent: agent
description: This prompt is used to end the current phase in the phased development workflow and prepare for a new phase in a new chat.
---

### Phase Transitions

- **Completion Check**: Before marking a phase as complete, ensure all related tasks are done.
- **Verification**:
  - Run `${workspaceFolder}/scripts/verify-phase.sh`. This script runs tests and clippy, and provides a checklist for the next steps.
  - **Epoch Check**: If this phase concludes an **Epoch**, ensure the "User Verification" steps defined in the plan have been manually verified by the user.
- **Meta-Review**: Update `${workspaceFolder}/AGENTS.md` with any new instructions or changes in workflow. If something didn't work well in this phase, fix the process now.
- **Coherence Check**: Verify that coherence between the documentation and codebase is increasing. If necessary, update documentation to reflect recent changes.
- **Walkthrough**: After all checks pass, update the `${workspaceFolder}/docs/agent-context/current/walkthrough.toml` file to reflect the work done since the last phase transition and surface it to the user for review.
- **Finish Phase**:
  - Run \`exo phase finish\` to mark the phase as completed in the plan.
- **Finalize**: Once the user has approved the walkthrough and the phase is marked complete:  - **RFC Promotion**: If this phase implemented a Stage 3 RFC, ensure `docs/manual/` is updated and the RFC is marked as Stage 4 (Stable).  - Run \`exo phase prepare\` (if available) or manually review \`docs/agent-context/future/\`.
  - Update `${workspaceFolder}/docs/agent-context/changelog.md`, `${workspaceFolder}/docs/agent-context/decisions.toml`, and `${workspaceFolder}/docs/agent-context/plan.toml`.
