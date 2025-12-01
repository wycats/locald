# Phase 11 Implementation Plan: Documentation & Persona Alignment

## Goal
Perform a "Fresh Eyes" review of the current documentation and codebase to ensure it aligns with our defined Personas (App Builder, Power User, Contributor). Fill in gaps and improve clarity before adding significant new complexity (Docker).

## User Requirements
- **App Builder**: Needs a frictionless "Getting Started" experience and clear examples for common tasks.
- **Power User**: Needs comprehensive reference documentation to understand capabilities and limitations.
- **Contributor**: Needs a clear mental model of the architecture to contribute effectively.

## Strategy
1.  **Review**: Audit existing docs against `docs/design/personas.md`.
2.  **Refine**: Rewrite or restructure confusing sections.
3.  **Expand**: Write missing guides or references.

## Step-by-Step Plan

### Step 1: Audit
- [ ] Review `locald-docs/src/content/docs/index.mdx` (Landing Page).
- [ ] Review `locald-docs/src/content/docs/guides/getting-started.md`.
- [ ] Review `locald-docs/src/content/docs/reference/configuration.md`.
- [ ] Identify gaps.

### Step 2: App Builder Focus
- [ ] Create/Update "Common Patterns" guide (e.g., "How to run a Node app", "How to run a Python app").
- [ ] Ensure error messages in the CLI are helpful (audit `locald-cli` output).

### Step 3: Power User Focus
- [ ] Ensure `locald.toml` reference is complete (including new `depends_on`).
- [ ] Document environment variables injected by `locald`.

### Step 4: Contributor Focus
- [ ] Update Architecture docs to reflect recent changes (State Persistence, Dependency Resolution).

### Step 5: Verification
- [ ] Build and preview the documentation site.
