---
agent: agent
description: This prompt is used to get a status report on the current phase.
---

### Phase Status Report

When asked for the status of the current phase, perform the following actions:

1.  **Gather Context**:
    - Run \`exo phase status\` to get a high-level overview of the current phase and task progress.
    - Read the current implementation plan artifact to understand the goals and scope.
    - Read `${workspaceFolder}/docs/agent-context/current/walkthrough.md` to see the narrative of progress so far, if it exists.

2.  **Generate Report**:

    - **Phase Identity**: State the current phase number and title.
    - **Progress Summary**: Summarize how much of the work is complete (e.g., "Design and Core Implementation are done, currently working on Tests").
    - **Pending Tasks**: List the immediate next tasks from `exo task list` when a phase is active (or `exo phase status` output).
    - **Blockers/Issues**: If the context suggests any open questions or deferred work, mention them.
    - **Next Action**: Suggest the logical next step for the user or agent.

3.  **Presentation**:
    - Keep the report concise and high-level.
    - Use bullet points for readability.
