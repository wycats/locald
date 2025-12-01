# Phase 8 Implementation Plan: Documentation Overhaul

## Goal
Restructure and expand the documentation to better serve distinct user personas, ensuring that `locald` is accessible to beginners while providing depth for power users and contributors.

## User Requirements
- **App Builder ("Regular Joe")**: Needs to know how to get started quickly. "How do I run my app?"
- **System Tweaker ("Power User")**: Needs to know how to configure ports, domains, and environment variables. "How do I change the port?"
- **Contributor ("The Rustacean")**: Needs to understand the internal architecture to contribute code. "How does the process manager work?"

## Strategy
1.  **Define Personas**: Explicitly write down who we are writing for in `docs/design/personas.md`.
2.  **Audit**: Review existing docs against these personas.
3.  **Restructure**: Organize docs into clear sections (Guides, Reference, Internals).
4.  **Fill Gaps**: Write missing content.

## Step-by-Step Plan

### Step 1: Foundation
- [ ] Create `docs/design/personas.md` with detailed descriptions of the three personas.
- [ ] Update `docs/design/README.md` to link to personas.

### Step 2: Structure & Audit
- [ ] Review `locald-docs/src/content/docs/` structure.
- [ ] Propose a new sidebar structure in `locald-docs/astro.config.mjs` (or wherever sidebar is defined).

### Step 3: Content Creation - App Builder
- [ ] Create/Update "Getting Started" guide.
- [ ] Create "Configuration Basics" guide (simple `locald.toml`).

### Step 4: Content Creation - Power User
- [ ] Create "Configuration Reference" (full `locald.toml` options).
- [ ] Create "CLI Reference" (all commands and flags).

### Step 5: Content Creation - Contributor
- [ ] Create "Architecture Overview" (diagrams/text).
- [ ] Create "Development Setup" guide.

### Step 6: Verification
- [ ] "Fresh Eyes" review: Read through the new docs as each persona.
