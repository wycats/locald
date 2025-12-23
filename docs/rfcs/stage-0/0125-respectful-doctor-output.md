---
title: "Respectful Doctor Output"
stage: 0 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Doctor Output
---

# RFC: Respectful Doctor Output

## 1. Summary

locald doctor output is a user interface, not a debug log. This RFC proposes a rendering model that is:

- Relevant to the default persona (App Builder)
- Actionable (the primary next action is obvious)
- Structured and copy/paste safe
- Compatible with docs/design/axioms/experience/04-output-philosophy.md

The first concrete improvement is fix consolidation, so when multiple failures share the same remediation (for example locald admin setup), the output communicates that clearly.

## 2. Motivation

Today, doctor can produce output where the same remediation is repeated per-problem and again in Suggested next steps. Even when the correct action is present, it can be hard to see that everything reduces to a single command.

This violates Respectful & Relevant Output by spending the user’s attention budget on duplication instead of intent.

## 3. Detailed Design

### Terminology

- Problem: a single check result (severity + status + summary + details).
- Remediation command: an executable command shown to the user.
- Fix group: a set of problems that share the same remediation command list.
- Persona:
  - Default: App Builder (what do I do next?)
  - Verbose: Contributor (what internal check failed and why?)

### User Experience (UX)

#### Output sections

Doctor output is organized into:

1. Context (strategy, cleanup mode)
2. Optional integrations (availability and impact)
3. Problems (failures and relevant details)
4. Fixes (consolidated, minimal)
5. Suggested next steps (small, ordered, structured list)

#### Consolidation: making the primary action obvious

When multiple problems share the same remediation, doctor should:

- Prefer a single consolidated Fix section over repeating the same Fix block under each problem.
- Show a short Resolves list under the consolidated command.

Before (current shape):

- Problems:
  - [FAIL] critical: locald cgroup root is not established
    Fix:
      - locald admin setup
  - [FAIL] critical: locald-shim does not match this locald build
    Fix:
      - locald admin setup

Suggested next steps:
- Install or repair…
  - locald admin setup

After (proposed shape):

Problems:
- [FAIL] critical: locald cgroup root is not established
  missing expected root: /sys/fs/cgroup/locald.slice
- [FAIL] critical: locald-shim does not match this locald build
  installed shim differs from embedded shim

Fix:
- Run: locald admin setup
- Resolves: cgroup root, locald-shim integrity/permissions

Suggested next steps:
- locald admin setup
- Next: run locald up.

#### Command normalization (copy/paste safety)

Remediation commands are UI strings and must be normalized:

- Prefer locald admin setup rather than sudo locald admin setup (avoid PATH restrictions; the command elevates internally).
- Expand known shorthands to full commands (for example admin setup → locald admin setup).
- Avoid formatting that can execute unexpectedly when copy/pasted.

### Architecture

Rendering is split into two phases:

1. Collect: produce a structured DoctorReport with problems and remediation metadata.
2. Render: map that structure to persona-appropriate human output.

Consolidation is a rendering concern and should not require changing the semantics of checks.

### Implementation Details

#### Consolidation algorithm

- For each problem with a non-empty remediation command list, normalize each command.
- Group problems by the normalized command list.
- If a group’s command list is shared by 2 or more problems, render it once in a consolidated Fix section.
- Problems in consolidated groups do not render per-problem Fix blocks.
- Problems whose remediation list is unique continue to render a per-problem Fix block.

#### Persona rules (initial)

- Default output should avoid internal check IDs unless they help the App Builder.
- Verbose output may include IDs, evidence, and additional details.

Exact flag naming is deferred; the current verbose flag is the likely bridge.

#### Crash protocol (future)

When doctor encounters an unexpected error that cannot be mapped to a domain-specific message, it should capture full diagnostic context, persist it to a crash log file, and print a concise message pointing to that file.

## 4. Implementation Plan (Stage 2)

- [ ] Add tests covering consolidation behavior for repeated remediation
- [ ] Implement fix grouping and consolidated rendering
- [ ] Ensure command normalization produces consistent output
- [ ] Adjust output to be persona-appropriate (default vs verbose)

## 5. Context Updates (Stage 3)

- [ ] Update manual docs for locald doctor output contract

## 6. Drawbacks

- Consolidation introduces output rules that must remain stable (tests required).
- Some users may prefer seeing remediation repeated under each failure; consolidation must still keep problems understandable.

## 7. Alternatives

- Keep per-problem remediation and only rely on Suggested next steps.
- Print a single global Fix banner but keep per-problem Fix blocks.

## 8. Unresolved Questions

- Should consolidation apply only when remediation lists are identical, or when one remediation is a superset of another?
- What are the final persona flags (beyond verbose) that match the output philosophy?
- Which internal identifiers (if any) should remain in default output?

## 9. Future Possibilities

- Introduce explicit persona flags (for example contributor, trace) instead of verbose.
- Implement crash log capture for unexpected errors (black-box rule).
- Add machine-readable JSON output stability guarantees for integrations.
