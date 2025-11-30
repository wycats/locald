# Implementation Plan - Phase 3: Documentation & Design Refinement

**Goal**: Establish a documentation site (Astro Starlight) to document the tool, serving the needs of our modes.

## High-Level Outline

### 1. Design Refinement (Completed)

- Firm up Interaction Modes & Personas.
- Fresh Eyes review of Axioms & Implementation.

### 2. Documentation Site Setup

- Initialize Astro Starlight project in `locald-docs/` using `pnpm create astro@latest`.
- Configure `astro.config.mjs` with `locald` title and description.
- Ensure the site runs with `pnpm dev`.

### 3. Content Creation

- **Landing Page (`index.mdx`)**: High-level overview and value prop.
- **Concepts (`concepts/modes.md`)**: Explain the 4 modes and personas.
- **Guides (`guides/getting-started.md`)**: Installation and first run.
- **Reference (`reference/configuration.md`)**: `locald.toml` options.
- **Reference (`reference/cli.md`)**: `locald` CLI commands.

## User Verification

- [ ] Verify the Astro Starlight site builds and runs locally.
- [ ] Verify the documentation content is readable and covers the core concepts.
