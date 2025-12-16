---
agent: agent
description: This prompt is used to start a new phase in the phased development workflow.
---

### Starting a New Phase

When starting a new phase in a new chat, you should restore the project context by following these steps:

- **Context Loading**: Make sure you understand the phased development workflow as described in this document.
- **State Verification**: Run \`exo context restore\`. This command will output the project goals, decisions, changelog, and current phase state. Read this output carefully to ground yourself in the project's history and current status.
- **Phase Activation**:
  - Identify the ID of the phase you are starting from \`docs/agent-context/plan.toml\` (or the output of \`exo context restore\`).
  - Run \`exo phase start <id>\` to mark the phase as active in the plan.
- **Plan Alignment**:
  - Consult `${workspaceFolder}/docs/agent-context/plan.toml` to identify the **Epoch** and **Phase** goals.
  - **RFC Selection**: Identify which Stage 1+ RFCs this phase implements. Add them to the plan (e.g., `rfcs = ["0030"]`).
  - Update `${workspaceFolder}/docs/agent-context/current/implementation-plan.toml` to be completely focused on the implementation plan for the next phase, deriving the high-level goals from the plan. Ask the user for feedback.
  - Ensure the Implementation Plan includes a **User Verification** section for manual checks.
  - Initialize `${workspaceFolder}/docs/agent-context/current/task-list.toml` with the tasks from the implementation plan.
  - Initialize `${workspaceFolder}/docs/agent-context/current/walkthrough.toml` with the goals and an empty "Changes" section.
- **Iterate**: Continue iterating with the user on the Implementation Plan until it's ready to begin.
