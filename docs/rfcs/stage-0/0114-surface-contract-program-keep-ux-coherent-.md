---
title: Surface Contract Program (Keep UX Coherent)
stage: 0
feature: Documentation / UX Coherence
---


# RFC 0114: Surface Contract Program (Keep UX Coherent)

## 1. Summary
This RFC records a durable program for making the *taught model* (docs/dashboard/CLI help) match the *required model* (what actually works), and preventing drift from reappearing.

It turns the audit approach in RFC 0112 into a scheduled sequence of phases with explicit outputs:

- a **Surface Contract** (what we are willing to teach as real)
- persona-aligned **Golden Paths** (what users do, when)
- a **stability labeling system** that governs what appears in docs/UI/help
- enforcement mechanisms so drift becomes difficult/impossible

This is a process/spec RFC. It does not change runtime behavior directly.

## 2. Motivation
We repeatedly hit drift between:
- docs and actual CLI surface (invented commands, legacy verbs)
- dashboard vocabulary and CLI vocabulary (vocabulary collisions)
- partially implemented features being presented as “real”

Drift causes support load and erodes user trust.

## 3. Goals
- Define a coherent model for what users do at various points in their journey.
- Consolidate and simplify the user-facing surface.
- Keep partially implemented features out of the “front door” until complete.
- Add CI checks so drift is caught automatically.

## 4. Non-Goals
- Implement new features here.
- Promote experimental surfaces into the front door without acceptance criteria.

## 5. Core Deliverables
### 5.1 Surface Contract v1
A canonical list of:
- verbs (one set of taught spellings)
- nouns (project/workspace/service/task)
- canonical remediation phrasing for privilege/readiness

### 5.2 Persona Golden Paths v1
At minimum:
- App Builder
- Power User
- Contributor

### 5.3 Stability Labels (governs docs + UI)
Every user-facing surface item is labeled:
- **Stable**: appears in Getting Started + CLI reference + dashboard
- **Experimental**: explicitly labeled; taught only in one place
- **Hidden**: may exist for contributors/tests; not taught
- **Removed**: not supported; migration notes only

### 5.4 Drift Prevention (“Contract Tests”)
- A generated CLI surface manifest validated in CI.
- A docs check that fails if user-facing docs reference non-manifest commands.
- A small glossary governance rule (new terms require reuse or an RFC).

## 6. Phase Schedule (Plan Integration)
After Phase 109 (host diagnostics), schedule the work as a dedicated epoch:

- Phase 110: Surface Contract v1 (Docs + Vocabulary Lock)
- Phase 111: CLI Surface Audit + Enforcement
- Phase 112: Dashboard Surface Audit
- Phase 113: Integration Boundaries Audit
- Phase 114: Feature Readiness Ledger + RFC Realignment

## 7. Relationship to RFC 0112
RFC 0112 is the audit artifact; this RFC is the durable program + schedule that ensures audit results translate into sustained product coherence.

## 8. Acceptance Criteria (Definition of Done)

These criteria define “done” for the scheduled phases in §6. They are intentionally scoped to documentation / surface coherence and drift prevention (no new runtime features).

### 8.1 Phase 110 — Surface Contract v1 (Docs + Vocabulary Lock)
- A single canonical “Surface Contract v1” exists (source of truth) that:
  - enumerates taught verbs and their canonical spellings (e.g. `up`, `status`, `doctor`, `admin setup`)
  - enumerates core nouns and their meanings (workspace/project/service/task) without ambiguity
  - defines the canonical remediation language for privilege/readiness problems
- The “front door” docs and CLI help text do not contradict the Surface Contract vocabulary.
- Any user-facing term that remains inconsistent is either:
  - corrected to match the contract, or
  - explicitly marked as experimental/legacy with a migration note.
- A short “What’s Stable vs Experimental” statement exists and is referenced from at least one entry doc (Getting Started or equivalent).

### 8.2 Phase 111 — CLI Surface Audit + Enforcement
- A machine-readable CLI surface manifest exists and can be regenerated deterministically (same inputs → same output).
- CI fails if user-facing docs reference CLI commands/options not present in the manifest.
- CI fails if the manifest generation changes without corresponding reviewable diffs (i.e. changes are visible in code review, not only at runtime).
- The enforcement is scoped strictly to taught surfaces (no requirements placed on internal/testing-only commands unless they are documented).

### 8.3 Phase 112 — Dashboard Surface Audit
- An inventory exists of what the dashboard teaches/surfaces (labels, navigation terms, feature names) with links to the Surface Contract nouns/verbs.
- Dashboard terminology matches the Surface Contract for the overlapping concepts.
- Any dashboard-only concept is either:
  - added to the contract (if truly part of the taught model), or
  - clearly labeled as experimental/internal and removed from “front door” documentation.
- No new dashboard capabilities are introduced as part of this phase; changes are vocabulary/labeling/information architecture cleanup only.

### 8.4 Phase 113 — Integration Boundaries Audit
- A stable/experimental matrix exists for integrations (e.g. OCI+shim/libcontainer readiness, CNB/buildpacks, VMM/KVM), including:
  - what happens when the integration is missing
  - what error/warning the user sees
  - the documented remediation path
- `locald doctor` surfaces integration availability in a way that is consistent with the matrix (warn vs critical as appropriate).
- Where the system already degrades gracefully, docs describe that behavior accurately; where it cannot, docs state the requirement plainly.

### 8.5 Phase 114 — Feature Readiness Ledger + RFC Realignment
- A “Feature Readiness Ledger” exists that maps each user-facing feature to:
  - stability label (Stable/Experimental/Hidden/Removed)
  - owning RFC(s) (or “no RFC yet”)
  - the canonical docs location (or “not documented”)
- Any feature that is taught as Stable has:
  - an up-to-date RFC at an appropriate stage (or an explicit decision that no RFC is needed)
  - docs that match the current implementation surface (no invented commands)
- A lightweight ongoing checklist exists describing how future work avoids drift (what to update when adding/removing commands/terms).
