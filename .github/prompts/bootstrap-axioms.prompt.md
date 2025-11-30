# Bootstrap Design Axioms

You are the **Chief Architect** and **Project Historian**. Your goal is to synthesize the fundamental "Design Axioms" of the project by analyzing the existing design documentation, decision logs, and codebase structure.

## Goal

Create or update `docs/design/axioms.md` to reflect the non-negotiable design principles that have emerged during development.

## Input Context

The user will provide (or you should read):

1.  `docs/design/*.md`: The free-form design thoughts.
2.  `docs/agent-context/decisions.md`: The history of architectural decisions.
3.  `AGENTS.md`: The core philosophy.

## Instructions

1.  **Analyze**: Read the provided documents. Look for:

    - Recurring patterns (e.g., "we always do X because of Y").
    - Hard constraints (e.g., "must support LSP").
    - Philosophical stances (e.g., "incremental by default").

2.  **Synthesize**: Group these findings into "Axioms". An Axiom is not just a good idea; it is a constraint that shapes the system.

3.  **Format**: Generate the content for `docs/design/axioms.md` using this format for each axiom:

    - **Principle**: A concise statement of the rule.
    - **Why**: The rationale.
    - **Implication**: The concrete effect on code or architecture.

4.  **Review**: Check if any existing design documents in `docs/design/` are now fully covered by these axioms and should be moved to `docs/design/archive/`.

## Output

1.  The content of `docs/design/axioms.md`.
2.  A list of design documents that can be archived (if any).
