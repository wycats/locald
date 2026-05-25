---
agent: agent
description: This prompt is used to end the current phase in the phased development workflow and prepare for a new phase in a new chat.
---

### Phase Transitions

- **Completion Check**: Before marking a phase as complete, ensure all related tasks are done.
- **Verification**:
  - Run `exo verify run`. This runs the phase verification defined by the project.
  - **Epoch Check**: If this phase concludes an **Epoch**, ensure the "User Verification" steps defined in the plan have been manually verified by the user.
- **Meta-Review**: Update `${workspaceFolder}/AGENTS.md` with any new instructions or changes in workflow. If something didn't work well in this phase, fix the process now.
- **Coherence Check**: Verify that coherence between the documentation and codebase is increasing. If necessary, update documentation to reflect recent changes.
- **Walkthrough**: After all checks pass, update `${workspaceFolder}/docs/agent-context/current/walkthrough.md` to reflect the work done since the last phase transition and surface it to the user for review.
- **Finish Phase**:
  - Run \`exo phase finish\` to mark the phase as completed in the plan.
- **Finalize**: Once the user has approved the walkthrough and the phase is marked complete:
  - **RFC Promotion**: If this phase implemented a Stage 3 RFC, ensure `docs/manual/` is updated before any Stage 4 promotion.
  - Run `exo plan review` to identify the next pending work.
  - Update `${workspaceFolder}/docs/agent-context/changelog.md` and `${workspaceFolder}/docs/agent-context/decisions.md` if there are narrative changes not already captured by `exo` or RFCs.
