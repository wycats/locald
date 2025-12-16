# Documentation Restructure Plan

**Goal**: Transform `docs.localhost` from a "random assortment of files" into a coherent, persona-driven documentation site that serves as the single entry point for all users.

## 0. Layering: Manual vs. Docs Site

We maintain two different kinds of documentation, and it is important that they do not drift:

- `docs/manual/` is the operational truth for contributors ("how it works now").
- `locald-docs/` is the published site (persona-driven and curated), but must remain consistent with the manual on user-facing behavior.

**Constraint**: When an RFC graduates into reality (implemented), the manual is updated first, and the published site is then reconciled for user-facing concepts.

## 1. Information Architecture (IA) Strategy

We will reorganize the site structure to map directly to our [Personas](../design/personas.md).

### The Landing Page (`/`)

**Goal**: Orient the user and route them to the right section immediately.

- **Hero**: Value proposition ("Local development for the 12-factor era").
- **Pathways**:
  - "I just want to run my app" -> **Guides (App Builder)**
  - "I want to configure my environment" -> **Reference (System Tweaker)**
  - "I want to understand how it works" -> **Internals (Contributor)**

### Section 1: Guides (The App Builder)

**Focus**: "Zero Friction", "It Just Works".

- **Getting Started**: Installation, First Run (`locald up`).
- **Core Workflows**:
  - Running a Web App (Port detection, `.localhost`).
  - Adding a Database (Managed Services).
  - Viewing Logs & Debugging.
- **Cookbooks**: Common patterns (Node, Rust, Python, Docker).

### Section 2: Reference (The System Tweaker)

**Focus**: Precision, Control, Completeness.

- **Configuration**: Complete `locald.toml` spec.
- **CLI**: Comprehensive command reference.
- **Execution Modes**: Host vs. Container (Deep dive).
- **Environment**: How `locald` manages `PATH`, `PORT`, and env injection.

### Section 3: Internals (The Contributor & Platform Engineer)

**Focus**: Architecture, Philosophy, Specifications.

- **Vision & Axioms**: The "Why" behind the project.
- **Architecture**: Process supervision, IPC, Shim security.
- **RFCs**: Design documents and decision logs.
- **Compliance**: CNB specs, OCI standards.

## 2. Content Migration Plan

We need to move content from `docs/manual` and `docs/design` into the `locald-docs` content structure (`src/content/docs`).

**Coherence checklist (high-risk drift areas):**

- Dashboard interaction model: Stream vs Deck pinning, plus the System Plane (pinning the virtual `locald` to open a Daemon Control Center).
- Domains: use `.localhost` consistently (avoid legacy `.local` except in historical context).

| Source                                          | Destination                    | Notes                                   |
| :---------------------------------------------- | :----------------------------- | :-------------------------------------- |
| `docs/manual/features/cli.md`                   | `reference/cli.md`             | Needs formatting for Starlight          |
| `docs/manual/features/execution-modes.md`       | `reference/execution-modes.md` |                                         |
| `docs/manual/features/managed-data-services.md` | `guides/databases.md`          | Rewrite as a "How-to"                   |
| `docs/design/axioms.md`                         | `internals/axioms.md`          | Move from front-and-center to Internals |
| `docs/design/vision.md`                         | `internals/vision.md`          |                                         |

## 3. Execution Steps

1.  **Scaffold Structure**: Create the new directory structure in `locald-docs/src/content/docs`.
2.  **Update Landing Page**: Rewrite `index.mdx` to feature the persona pathways.
3.  **Migrate & Refine**: Move content file-by-file, rewriting headers and links.
4.  **Axiom Integration**: Ensure Axioms are referenced in Guides (e.g., link to "Zero Friction" in Getting Started) but live in Internals.
5.  **Verify**: Build the site and check navigation.
6.  **Fresh Eyes Review**: Conduct a final review of the implementation plan and documentation to ensure alignment with the current codebase.

## 4. Future Work

- **Search**: Ensure Starlight search is configured correctly.
- **Versioning**: Plan for versioned docs (v1, v2) if needed later.
