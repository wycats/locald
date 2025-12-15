---
agent: agent
description: Verifies that the current chat session's progress is properly serialized into the agent context for the next session.
---

# Context Serialization Check

You are about to end the current session or transition to a new chat. Your goal is to ensure that the "Living Context" is up-to-date so that the next agent can pick up exactly where you left off without reading the chat history.

## The Golden Rule

**"If it isn't in the context files, it didn't happen."**

## Serialization Checklist

Review the current chat session and ensure the following information is serialized to disk:

1.  **Decisions**: Did we agree on a specific approach, library, or pattern?
    - _Target_: `docs/agent-context/decisions.md` (or relevant RFCs).
2.  **Progress**: What specific steps did we complete?
    - _Target_: `docs/agent-context/current/walkthrough.md` (Update the "Changes" or "Walkthrough" section).
3.  **State**: What is the immediate next step?
    - _Target_: `docs/agent-context/current/task-list.md` (Mark completed tasks, add new ones).
4.  **Ideas & Deferrals**: Did we have ideas we didn't implement? Did we skip something?
    - _Target_: `docs/agent-context/future/ideas.md` or `deferred_work.md`.

## Instruction

If you find any "Apocrypha" (knowledge existing only in this chat), **promote it to Canon** by updating the relevant context files immediately.
