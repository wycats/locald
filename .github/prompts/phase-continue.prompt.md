---
agent: agent
description: This prompt is used to resume work on an existing phase in a new chat session.
---

### Continuing a Phase

When picking up work in the middle of a phase (e.g., starting a new chat session for an ongoing task), follow these steps:

- **Context Restoration**: Run \`exo context restore\`.
  - This command will output the current project context, including the active task list and implementation plan.
- **State Analysis**:
  - Review `exo task list` when a phase is active to identify completed and pending items.
  - Review the implementation plan artifact to understand the current technical direction.
  - Review `docs/agent-context/current/walkthrough.md` (if any) to see what has been accomplished so far.
- **Resume Work**:
  - Identify the next incomplete task from the task list.
  - Continue execution from that point.
