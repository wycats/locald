---
title: Property Tests as Invariant Specs (Doctor/Fix Consolidation)
stage: 0
feature: Engineering Excellence
---

# RFC 0111: Property Tests as Invariant Specs (Doctor/Fix Consolidation)

## 1. Summary

Codify a testing pattern: use property-based tests as executable specifications for invariants that must never drift.

This RFC is motivated by the new host diagnostics surface (RFC 0109) and privileged capability acquisition report (RFC 0110), where we want:

- correctness that is hard to “spot check” (fix consolidation soundness, ordering, and dominance)
- stability of canonical remediation (avoid accidentally changing the one blessed command)
- confidence without relying on host-dependent integration tests

This RFC is Stage 0 because it proposes workflow + documentation changes and a small set of additional invariants to lock down.

## 2. Motivation

The “doctor report” and “capability acquisition report” are a user-facing contract.
Regressions here are expensive because they surface as:

- misleading advice (invented or dropped fix steps)
- shifting canonical remediation (“do X”, then next week “do Y”)
- environment-sensitive flakes (tests that pass locally but fail in CI or containers)

Property tests are a good fit when:

- we can describe correctness as universal properties (soundness, normalization, monotonicity)
- we can generate many edge-cases cheaply
- the code can be factored to expose pure helpers

## 3. Detailed Design

### 3.1. Invariant Taxonomy (What we proptest)

**A. Soundness (no invention)**

- Consolidated `FixAdvice` must come from failing `Problem`s.
- A consolidation pass may _dedupe_ and _reorder_, but it must not create new fix keys out of thin air.

**B. Canonical remediation drift prevention**

- `FixKey::RunAdminSetup` must imply commands are exactly: `sudo locald admin setup`.
- It must not grow optional “bonus commands” over time.

**C. Priority dominance / ordering**

- If `RunAdminSetup` is present at all, it must be first in consolidated fixes.
- No lower-priority fix should appear before it.

**D. Structural safety invariants**

- Sanitizers and path constructors must never emit unsafe components (empty, `..`, `//`, unexpected separators).
- Strategies derived from environment probes should be routed through small pure helpers that are unit/proptest-friendly.

### 3.2. Fix Semantics: Precedence, Not Subsumption

Fix consolidation is a _presentation_ and _deduplication_ step, not a suppression step.

Rule:

- `FixKey::RunAdminSetup` (and any other “canonical fix”) must **precede** other fix keys.
- It must **not subsume** them.

Rationale:

- `sudo locald admin setup` can repair installation state (shim present/permissions, cgroup root establishment), but it cannot necessarily fix host policy or environmental constraints (e.g. `nosuid`, LSM confinement, unprivileged containers). Those must remain visible.

### 3.3. Semantic Invariants (The “Laws”)

Once precedence-not-subsumption is the rule, we can state stronger invariants.

Let $K$ be the set of fix keys attached to **failing** problems.
Let $F$ be the fix keys emitted by `consolidate_fixes`.

We want:

- **Soundness**: $F \subseteq K$ (never invent advice).
- **Completeness**: $K \subseteq F$ (never drop advice).
- Therefore **Equality**: $F = K$ (modulo dedupe and stable ordering).

And:

- **Dominance**: if `RunAdminSetup ∈ F`, it appears first.
- **Canonical commands**: if `RunAdminSetup ∈ F`, its command list is exactly `["sudo locald admin setup"]`.

### 3.4. Pattern: “Extract Pure Helper, Then Proptest It”

For environment-dependent logic (e.g. PID 1 comm, filesystem probes), define a pure function that maps a small input into a decision:

- `root_strategy_from_pid1_comm(comm: &str) -> CgroupRootStrategy`

Then proptest the mapping for broad inputs, and separately unit test the integration layer that reads the real system.

### 3.5. Pattern: “Fix Consolidation as a Spec”

Fix consolidation is a classic place where bugs hide:

- duplicates
- missing fix in presence of multiple problems
- contradictory orderings
- accidental drift in canonical fix text

Model it with keys (`FixKey`) and keep consolidation logic free of IO.

### 3.6. Evidence & Workflow (Did the proptest catch a bug?)

Property tests are most valuable when they fail _before_ the code is corrected.
To avoid folklore, adopt a lightweight evidence rule:

- When a proptest finds a counterexample, record it (seed/case) and link it in the walkthrough or PR description.
- Prefer converting discovered counterexamples into small deterministic unit tests where practical.

Implementation detail depends on the chosen `proptest` regression strategy (e.g. storing regression files vs. inlining cases).

### 3.7. If We Ever Want Subsumption

If we later decide that some fix keys should subsume others, that must be an explicit, documented lattice.

At that point the invariants must be updated (e.g. completeness becomes “complete under subsumption rules”), and tests should encode the lattice, not the incidental implementation.

## 4. Implementation Plan (Stage 2)

- [ ] Audit git history to determine whether any proptests actually failed first (and if so, capture the counterexample).
- [ ] Add invariants for `FixKey::RunAdminSetup`:
  - [ ] dominance: first when present
  - [ ] canonical command list: exactly `sudo locald admin setup`
- [ ] Add semantic invariants for consolidation:
  - [ ] completeness over failing fix keys ($K \subseteq F$)
  - [ ] monotonicity under added failures (adding failures cannot remove fix keys)
- [ ] Optionally add semantic properties (if they clarify contract rather than restate code):
  - [ ] report stability: JSON roundtrip preserves fix keys + severity classification

## 5. Documentation / Law Updates (Stage 3)

- [ ] Update manual docs to include “Property Tests as Invariants” guidelines:
  - extract pure helpers
  - prefer soundness/normalization properties
  - avoid host IO in proptests
  - record counterexamples when discovered
- [ ] Update relevant RFCs to reference the invariant approach:
  - RFC 0109: doctor is a contract; advice drift prevention
  - RFC 0110: fix consolidation must be sound and complete; canonical remediation should be stable

## 6. Drawbacks

- Property tests can be harder to debug than unit tests.
- Bad generators create false confidence (low entropy) or slow test suites.
- Some properties (monotonicity under subsumption) require an explicit spec.

## 7. Alternatives

- Only unit tests: easier to read, but weaker at boundary coverage.
- More integration tests: more realistic, but flaky and host-dependent.
- Snapshot tests of rendered doctor output: catches drift, but brittle and discourages improving messaging.

## 8. Unresolved Questions

- Where should counterexamples live long-term?
  - proptest regression files vs. curated unit tests.

## 9. Future Possibilities

- A small “invariant catalog” page in the manual that lists core contracts:
  - doctor report stability
  - fix consolidation soundness + completeness
  - canonical remediation drift prevention
