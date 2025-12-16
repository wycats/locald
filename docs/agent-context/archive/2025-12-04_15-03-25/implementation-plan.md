# Implementation Plan - Phase 25: Roadmap & Design Organization

## 1. Inventory
- List all files in `docs/design`.
- Categorize them by status (Implemented, Planned, Idea, Deprecated).
- Identify "orphan" design docs that don't map to a phase.

## 2. Prioritization
- Review the current `plan-outline.md`.
- Discuss with the user:
    - Are the Epochs still the right high-level grouping?
    - Is the order of phases within Epoch 3 and 4 correct?
    - Are there missing phases based on the design docs?
    - Are there design docs that should be archived?

## 3. Restructure
- Create a folder structure in `docs/design` that mirrors the Epochs/Phases.
    - `docs/design/epoch-1-mvp/` (Archive)
    - `docs/design/epoch-2-refinement/` (Archive)
    - `docs/design/epoch-3-hybrid/` (Active)
    - `docs/design/epoch-4-build/` (Future)
- Move existing design docs into the appropriate folders.
- Update links in `plan-outline.md` and other docs.

## 4. Plan Update
- Update `docs/agent-context/plan-outline.md` with the refined roadmap.
- Ensure every active phase has a corresponding design doc (or a placeholder).
