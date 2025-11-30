---
agent: agent
description: This prompt is used to start a new phase in the phased development workflow.
---

### Starting a New Phase

When starting a new phase in a new chat, you should restore the project context by following these steps:

- **Context Loading**: Make sure you understand the phased development workflow as described in this document.
- **State Verification**: Run `${workspaceFolder}/scripts/agent/restore-context.sh`. This script will output the project goals, decisions, changelog, and current phase state. Read this output carefully to ground yourself in the project's history and current status.
- **Plan Alignment**:
  - Consult `${workspaceFolder}/docs/agent-context/plan-outline.md` to identify the **Epoch** and **Phase** goals.
  - Update `${workspaceFolder}/docs/agent-context/current/implementation-plan.md` to be completely focused on the implementation plan for the next phase, deriving the high-level goals from the outline. Ask the user for feedback.
  - Ensure the Implementation Plan includes a **User Verification** section for manual checks.
  - Initialize `${workspaceFolder}/docs/agent-context/current/task-list.md` with the tasks from the implementation plan.
  - Initialize `${workspaceFolder}/docs/agent-context/current/walkthrough.md` with the goals and an empty "Changes" section.
- **Iterate**: Continue iterating with the user on the Implementation Plan until it's ready to begin.
