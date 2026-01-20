# Ship-Readiness Review Plan (locald)

## Purpose

Define a concrete path to an announcement-ready locald by auditing code and documentation, identifying blockers, and cutting scope to a shippable core.

## Operating Principles

- Ship the fundamentals first.
- Cut or hide anything experimental or unfinished.
- Everything public must be testable, documented, and recoverable.

## Status Update (2026-01-20)

- Drafted Stage-0 RFCs: WSL Windows helper, macOS domain/cert requirements, dashboard vocabulary.
- Manual docs drift fixes: removed legacy `locald start` mentions; added experimental/status banners for CNB/container/ephemeral/plugins and `locald ai` proposal.

## Phase 1: Define the Shippable Core (Owner: Main)

### Objectives

- Establish the “Day 1 Story” and what is in/out of v1.0.

### Outputs

- PITCH.md draft: 1-sentence value proposition.
- Hello World demo path: locald up → working service in <30 seconds.
- Persona focus list (ranked): who v1.0 is for.
- Scope boundary: IN / OUT / HIDE lists.

### Acceptance Criteria

- The pitch and demo align with current functionality.
- No experimental features appear in public claims.

## Phase 2: Codebase Audit (Owner: Subagents)

### Goal

Classify major modules as SHIP / DEFER / REMOVE with landmines.

### Subagent Assignments

- Core Audit: crates/locald-core, crates/locald-server, crates/locald-cli
- Frontier Audit: crates/locald-vmm, plugin/distribution/WASM systems
- Surface Audit: CLI surface, error messages, help output
- Docs Audit: locald-docs, README, guides, reference

### Subagent Output Format (JSON)

- module
- public_functions
- items: [{name, status: SHIP|DEFER|REMOVE, rationale}]
- landmines: [path, symptom, impact]
- recommendations: [short actionable fixes]

## Phase 3: Pre-Mortem (Owner: Main)

### Prompt

Assume the announcement failed. List the top reasons.

### Failure Modes to Test

- “locald up” fails with unclear errors.
- README fails the “stranger test.”
- Unfinished features visible in docs or CLI.
- Mismatch between marketing and reality.

### Deliverable

- Pre-mortem report with failure triggers and mitigations.

## Phase 4: Gap Analysis (Owner: Main + Subagents)

### Checklist

- Installation path: binary, install.sh, upgrade path.
- Onboarding: 5-minute tutorial that actually works.
- Recovery: “doctor” and error recovery flows.
- Docs coverage: all shipped features documented, no stale claims.

### Deliverable

- Gap list with severity and owner.

## Phase 5: Scope Razor (Owner: Main)

### Rules for IN

- Has tests.
- Has docs.
- Proven by real daily usage.
- Graceful error paths.

### Rules for OUT

- Experimental or unfinished.
- Missing tests/docs.
- Requires special flags or awkward setup.

### Deliverable

- v1.0 Manifest: final “IN / OUT / HIDE” list.

## Execution Order

1. Define Shippable Core
2. Run subagent audits in parallel
3. Synthesize results + Pre-mortem
4. Gap analysis
5. Final v1.0 Manifest
