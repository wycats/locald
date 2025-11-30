---
agent: agent
description: This prompt is used to prepare the next phase in the phased development workflow.
---

### Preparation

- The `complete-phase-transition.sh` script will have displayed the contents of `docs/agent-context/future/`. Review this output and the chat history.
- Propose a high-level outline for the next phase to the user.
- Once the user has approved the high-level outline, update `${workspaceFolder}/docs/agent-context/current/implementation-plan.md` with the agreed outline. Do not include detailed implementation steps yet.
- Update `${workspaceFolder}/docs/agent-context/plan-outline.md` to reflect the portion of the outline that will be tackled in the next phase.
- Update `${workspaceFolder}/docs/agent-context/future/` files to remove any items that will be addressed in the next phase, and add any new ideas or deferred work that arose during the iteration with the user.
