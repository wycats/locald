---
title: Cross-Surface Workflow Contracts
stage: 0
feature: Engineering Excellence
---


# RFC 0105: Cross-Surface Workflow Contracts

## 1. Summary

Define a lightweight, enforceable set of workflow contracts for changes that span multiple surfaces:

- product UI (dashboard)
- documentation (docs site)
- automation (Playwright screenshots, scripts, CI checks)

The goal is to reduce churn by making "what must not break" explicit, testable, and easy to verify.

## 2. Motivation

Cross-surface projects tend to churn because they have misaligned stability requirements:

- Docs need stable URLs, stable information architecture, and stable assets.
- Product UI needs iteration velocity.
- Visual tests need determinism.

When these contracts are implicit, we rediscover them via regressions (broken links, flaky snapshots, duplicated nav entries) and debugging time shifts to the wrong layer (pixel diffs instead of state invariants).

This RFC proposes a repeatable pattern that makes cross-surface work predictable.

## 3. Detailed Design

### 3.1. Interface Contract (The "Must Not Break" List)

For any cross-surface change, maintain a short contract that explicitly defines:

- Stable URLs: canonical docs origin (e.g. http://docs.localhost) and any documented service origins (e.g. https://dev.locald.localhost/).
- Stable asset targets: docs must link to stable, served screenshot URLs (e.g. /screenshots/*.png).
- Stable routes: docs routes remain stable; labels can change.
- Generated vs authored boundaries: generated content trees are not edited by hand; authored content is not overwritten.

This contract is small enough to live in the manual and be checked by automation.

### 3.2. Semantic Anchors (Test IDs as Public API)

Treat a small set of semantic selectors as part of the tooling API:

- data-testid for major modes (e.g. Stream vs Deck)
- data-testid for major panels (e.g. daemon/system plane)

Rule of thumb: each new "surface" (mode/panel/page) ships with 2-3 stable anchors.

### 3.3. Two-Lane Artifact Pipeline

Separate screenshot workflows into two lanes:

- Lane A (Authoritative): Playwright snapshot baselines reviewed/approved, then synced into the docs public directory as stable files.
- Lane B (Ad hoc): manual capture for exploration/debugging; never referenced by docs.

Docs should only consume Lane A artifacts.

### 3.4. Triage Order (Stop Debugging in the Wrong Layer)

When a cross-surface check fails, enforce a triage order:

1. Verify state invariants via DOM snapshots / explicit assertions (e.g. "System is pinned -> Deck mode visible").
2. Verify product state transitions (routing, mode toggles, binding/reactivity).
3. Only then evaluate pixel diffs.

### 3.5. Stop Points (Required Audits)

Introduce explicit stop points for cross-surface PRs:

- docs build passes
- screenshot suite passes (or baselines updated with review)
- link/asset checks pass (e.g. any referenced /screenshots/*.png exists)
- navigation sanity checks pass (e.g. no duplicate sidebar links)

## 4. Implementation Plan (Stage 2)

- [ ] Add a short "Cross-Surface Project Playbook" page under docs/manual/ (reality + checklist).
- [ ] Add/standardize data-testid anchors for:
  - [ ] Stream root
  - [ ] Deck root
  - [ ] System/daemon panel root
- [ ] Add checks:
  - [ ] detect duplicate docs sidebar links
  - [ ] verify referenced /screenshots/*.png exist in the docs public directory
- [ ] Wire checks into the repo's standard check path (scripts/check or equivalent).

## 5. Context Updates (Stage 3)

- [ ] Update docs/manual/ to include the contract checklist.
- [ ] Update contributor docs to explain Lane A vs Lane B screenshot artifacts.

## 6. Drawbacks

- Introduces lightweight process overhead (anchors, checks, stop points).
- Requires discipline to keep the contract small and relevant.
- Test IDs become a supported interface for tooling and must be maintained.

## 7. Alternatives

- Keep process implicit and rely on human review.
- Split work by team boundaries (docs vs product) and accept slower iteration.
- Avoid visual regression entirely and rely on unit/integration tests.

## 8. Unresolved Questions

- What is the minimal set of anchors that still provides robust invariants?
- Should screenshot stability prefer masking dynamic regions, or a dedicated "screenshot mode" (e.g. query param) that freezes dynamic data?
- What should be required in CI vs only recommended locally?

## 9. Future Possibilities

- A dedicated "screenshot mode" to disable streaming log output and freeze timestamps.
- A small "cross-surface PR template" that links the built docs + screenshot artifacts.
