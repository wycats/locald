# Documentation Restructure

- Feature Name: `documentation_restructure`
- Start Date: 2025-12-08
- RFC PR: (leave empty)
- Tracking Issue: (leave empty)

## Summary

Transform `docs.localhost` from a "random assortment of files" into a coherent, persona-driven documentation site that serves as the single entry point for all users. The structure will be reorganized into four main sections: **Guides** (App Builder), **Concepts** (Curious User), **Reference** (System Tweaker), and **Internals** (Contributor).

## Motivation

Currently, the documentation is scattered across `docs/manual`, `docs/design`, and `locald-docs`. Users often struggle to find relevant information because it's not organized by their intent or persona. A "Regular Joe" just wants to run an app, while a "Power User" needs configuration details, and a "Contributor" wants to understand the architecture. Mixing these concerns leads to confusion.

## Guide-level explanation

The documentation site will be reorganized into four distinct sections, accessible from the landing page:

1.  **Guides (The App Builder)**: Focused on "Zero Friction" and "It Just Works". This section contains the "Getting Started" guide, core workflows (running apps, adding databases), and cookbooks for common patterns.
2.  **Concepts (The Curious User)**: Focused on "Mental Models" and "Philosophy". This section explains the "Why" and "What" of `locald`—Vision, 12-Factor Local, and the Workspace model—without getting bogged down in implementation details.
3.  **Reference (The System Tweaker)**: Focused on precision and control. This section contains the complete `locald.toml` configuration specification, the CLI command reference, and technical details on Health Checks and Execution Modes.
4.  **Internals (The Contributor)**: Focused on architecture and implementation. This section contains the project axioms, architectural deep dives (IPC, Shim), and RFCs.

The landing page (`index.mdx`) will be redesigned to route users to these sections based on their needs.

## Reference-level explanation

The `locald-docs` directory structure (Astro Starlight) will be updated to reflect this hierarchy:

```
src/content/docs/
├── guides/
│   ├── getting-started.md
│   ├── databases.md
│   └── ...
├── concepts/
│   ├── vision.md
│   ├── 12-factor.md
│   └── ...
├── reference/
│   ├── configuration.md
│   ├── cli.md
│   ├── health-checks.md
│   └── ...
├── internals/
│   ├── architecture.md
│   ├── rfcs/
│   └── ...
└── index.mdx
```

Content from `docs/manual` and `docs/design` will be migrated to this new structure.

## Rationale and alternatives

- **Persona-Driven**: Aligning documentation with personas ensures that users see the information relevant to them without being overwhelmed by details they don't need.
- **Single Entry Point**: Consolidating everything into `docs.localhost` makes it the authoritative source of truth.

Alternative: Keep the current flat structure. This was rejected because it scales poorly as the project grows.

## Prior art

- **Rust Documentation**: Separates "The Book" (Guide), "Reference" (Reference), and "Nomicon" (Internals).
- **Diátaxis Framework**: Advocates for separating tutorials, how-to guides, reference, and explanation.

## Unresolved questions

- How to handle versioning of documentation? (Deferred to future work)
