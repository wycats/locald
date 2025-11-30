---
agent: agent
description: This prompt is used to resume work on an existing phase in a new chat session.
---

### Continuing a Phase

When picking up work in the middle of a phase (e.g., starting a new chat session for an ongoing task), follow these steps:

- **Context Restoration**: Run `${workspaceFolder}/scripts/agent/resume-phase.sh`.
  - This script will verify that an active phase exists and output the current project context.
- **State Analysis**:
  - Review the `Task List` output to identify completed and pending items.
  - Review the `Implementation Plan` to understand the current technical direction.
  - Review the `Walkthrough` (if any) to see what has been accomplished so far.
- **Resume Work**:
  - Identify the next incomplete task from the task list.
  - Continue execution from that point.
