# Cross-Surface Projects (UI + Docs + Automation)

Cross-surface work is any change that touches more than one of:

- the dashboard UI
- the docs site
- automation (scripts, Playwright, CI)

The failure mode is predictable: each surface wants a different kind of stability.
This playbook defines contracts that keep iteration fast while keeping breakage obvious.

## The Interface Contract (Must Not Break)

Before you start, write down the contract for the change. Keep it short.

- **Stable routes**: docs routes stay stable; labels/copy can change.
- **Stable origins**: docs canonical URL is `http://docs.localhost`.
- **Stable screenshot URLs**: docs link to `/screenshots/*.png` (served from `locald-docs/public/screenshots/`).
- **Generated vs authored**:
  - generated docs trees are never edited by hand
  - authored docs are never overwritten by sync scripts

If the contract changes, update it first.

## Semantic Anchors (Test IDs are Tooling API)

Cross-surface work should ship stable anchors for tooling.

Minimum anchors:

- **Mode roots**: Stream vs Deck (so tests can assert state before pixels).
- **Major panels**: System/daemon panel (so tests don’t scrape incidental UI).

Rule of thumb: each new “surface” (mode/panel/page) ships with 2–3 stable `data-testid` values.

## Two-Lane Screenshot Pipeline

Screenshots have two lanes:

- **Lane A (authoritative)**: Playwright baselines are reviewed/approved, then synced into docs as stable PNGs.
- **Lane B (ad hoc)**: manual capture for exploration/debugging; never referenced by docs.

Docs must only consume Lane A.

## Triage Order (Don’t Debug Pixels First)

When a visual check fails:

1. Assert **state invariants** first (DOM/anchors): “pinned => Deck visible”, “System panel visible”.
2. Debug state transitions (routing, bindings, reactivity).
3. Only then inspect pixel diffs.

## Stop Points (Required Audits)

Before merging cross-surface PRs:

- docs build passes
- screenshot tests pass (or updated baselines are reviewed)
- docs nav has no duplicated links
- all referenced `/screenshots/*.png` exist in the docs public directory
