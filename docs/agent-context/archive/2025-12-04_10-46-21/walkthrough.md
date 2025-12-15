# Walkthrough: Design Restructuring & Vision

## Goal
The goal of this phase was to elevate `locald` from a "process runner" to a "Local Development Platform" by refining the project vision and restructuring the design axioms into a cohesive manifesto.

## Changes

### 1. The Manifesto
We reorganized the design documentation into three core pillars:
-   **Experience**: Zero-friction start, Workspace dashboard, Interface parity.
-   **Architecture**: Daemon-first, Process ownership, Privilege separation, Ephemeral runtime.
-   **Environment**: Structured hierarchy, Managed networking, Portability.

This structure is now reflected in `docs/design/axioms.md` and the file system.

### 2. New Design Concepts
-   **Vision**: Created `docs/design/vision.md` to articulate the "Localhost as a Platform" philosophy.
-   **Ephemeral Runtime**: Added **Axiom 7**, defining how the system preserves context (logs, history) even when processes crash.
-   **Dashboard Ergonomics**: Created `docs/design/dashboard-ergonomics.md` to plan the transformation of the dashboard into an interactive workspace (Phase 24).
-   **Advanced Proxying**: Created `docs/design/advanced-proxying.md` to outline future support for complex routing and modern protocols (H2/H3).

### 3. Documentation Workflow
-   Implemented a `sync-manifesto` script and `lefthook` integration.
-   Design documents in `docs/design` are now automatically synced to the `locald-docs` site, ensuring the "Manifesto" section is always up to date without manual copy-pasting.

## Verification
-   [x] `docs/design/axioms.md` links are valid.
-   [x] `locald-docs` builds successfully with the synced manifesto.
-   [x] The vision aligns with the user's feedback on "Constellations" and "Workspace".
