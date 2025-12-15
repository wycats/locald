---
title: Codebase Cleanup
stage: 0
feature: Engineering Excellence
---

# RFC 0104: Codebase Cleanup

## 1. Summary

This RFC proposes a focused cleanup pass that improves review readiness for experienced Rust developers.

The goal is not to change the architecture, but to remove misleading signals, tighten consistency between code/comments/tooling, and make “how to validate” match CI.

## 2. Motivation

We want external Rust reviewers to be able to:

- Understand the project shape in ~5 minutes.
- Run the same checks CI runs without spelunking.
- Trust that lint comments and safety claims match reality.

This cleanup is also explicitly aimed at removing “false confidence” artifacts (e.g. comments that assert properties that aren’t true) and reducing style inconsistencies that read as machine-generated.

## 3. Detailed Design

### 3.1 Success Criteria

1. A repository “front door” exists at the root (README) with:
	- What `locald` is
	- Where to start reading
	- The canonical CI-equivalent local validation commands
2. Comments about safety/lints are accurate (no contradictions like “no panics” while allowing `unwrap`).
3. Repo scripts follow the operational constraints documented in `AGENTS.md`:
	- sandboxed daemon operations
	- no `kill`/`pkill` usage
4. CI remains green (and local instructions reproduce CI behavior).

### 3.2 Non-Goals

- Major architectural refactors.
- Large-scale module reorganizations (unless they directly improve reviewer comprehension).
- Eliminating all `unwrap/expect` everywhere (tests/examples may keep them; production paths should be audited).

### 3.3 Improvements (Full List)

#### A. “Front Door” for Reviewers

1. Add a root-level README.
	- Provide a 90-second overview and a “start here” map (CLI/server/core/shim/dashboard/docs).
2. Provide a single canonical “validate like CI” section.
	- Primary: run the exact CI-aligned scripts/commands (see Validation).
3. Document the privileged shim requirement in one obvious place.

#### B. Validation Should Match CI

1. Make it explicit that CI is the contract and local validation should reproduce it.
2. Ensure local scripts do not drift from `.github/workflows/ci.yml`.
	- Prefer keeping `scripts/ci-rust-checks-local.sh` aligned with CI (it already states this intent).
3. Decide what `scripts/check` is:
	- Option 1: make it CI-equivalent.
	- Option 2: make it a fast developer sanity check, and rename it to avoid implying CI equivalence.

Current mismatch inventory:

- `scripts/check`:
  - Uses `kill` on failure (conflicts with `AGENTS.md` process lifecycle constraint).
  - Starts a daemon without `--sandbox`.
  - Runs `pnpm check` for dashboard, while CI runs `pnpm build`.

#### C. Sandbox & Daemon Lifecycle Consistency

1. Any script that starts a daemon uses `--sandbox=<name>`.
2. Failure paths do not use `kill`; use `locald server shutdown --sandbox=<name>` and `wait`.
3. Use a consistent sandbox naming scheme:
	- `--sandbox=prepush` for pre-push tooling
	- `--sandbox=ci` for CI
	- `--sandbox=check` for local “scripts/check”

#### D. Lint Policy & Comment Accuracy

The cleanup pass should specifically address contradictions that undermine trust.

1. Align lint declarations and their comments.
	- Example: `locald-server/src/lib.rs` currently has:
	  - `#![allow(clippy::unwrap_used)] // Force error propagation (no panics)`
	  - This is contradictory and should be corrected.
2. Reduce crate-level `allow(...)` usage where possible.
	- Prefer narrow scope allowances with a reason and a tracking reference.
3. Ensure “ban println” policies match actual code expectations.
	- If a crate uses CLI-style stdout printing intentionally, don’t label it as banned.

#### E. Panic/unwrap/expect Audit Policy

We should be explicit about where `unwrap/expect` is acceptable.

1. Define a tiered policy:
	- **Tier 1 (Production paths):** prefer error propagation; avoid `unwrap/expect`.
	- **Tier 2 (Build scripts / tooling):** `expect` is acceptable with precise messages.
	- **Tier 3 (Tests/examples):** `unwrap/expect` is acceptable.
	- **Tier 4 (Experimental crates):** may allow more, but should be labeled as such.
2. Use a mechanical audit to track remaining instances.

Initial hotspots (non-exhaustive examples):

- `locald-vmm/src/linux.rs`: many `expect(...)` plus `println!(...)` (may be fine if treated as experimental).
- `locald-core/src/config/mod.rs`: `unwrap` used in tests/helpers.

#### F. Observability Consistency

1. Ensure long-running services use `tracing` consistently.
2. Ensure CLI stdout printing is deliberate and separated from daemon logging.
3. Ensure test harness logging is gated or clearly test-only.

#### G. Documentation Hygiene That Impacts Review

1. Root README points at the most useful deep references:
	- `docs/manual/architecture/`
	- `docs/manual/features/`
	- “how to run CI locally”
2. Verify rustdoc metadata URLs are intentional and not stale branch references.
	- If they must be URLs, prefer a stable branch (e.g. default branch) or a stable asset location.

#### H. “AI-smell” Prevention Guardrails (Process)

This is not a stylistic crusade. The goal is to remove high-suspicion patterns:

1. Delete or rewrite comments that claim properties not enforced by code.
2. Prefer small, local explanations (“why this is safe”) over broad assertions.
3. Prefer consistent naming and error patterns over cleverness.
4. Prefer tests that encode invariants over prose.

### 3.4 Audit Commands (for maintaining the “full list”)

These commands produce an objective inventory that can be used to track progress:

- `rg -n "\\.unwrap\\(" --glob '!**/target/**'`
- `rg -n "\\.expect\\(" --glob '!**/target/**'`
- `rg -n "\\bdbg!\\(" --glob '!**/target/**'`
- `rg -n "\bprintln!\(|\beprintln!\(" --glob '!**/target/**'`
- `rg -n "allow\\(clippy::|deny\\(clippy::|warn\\(clippy::" --glob '**/*.rs'`
- `rg -n "\bunsafe\b" --glob '**/*.rs'`

## 4. Implementation Plan (Stage 2)

Implementation should proceed in small PR-sized steps.

- [ ] Add root README with “Start Here” + CI-equivalent commands.
- [ ] Decide role of `scripts/check` (CI-equivalent vs fast sanity) and align it accordingly.
- [ ] Update scripts that start/stop daemons to always use `--sandbox` and avoid `kill`.
- [ ] Fix contradictory lint comments (start with `locald-server/src/lib.rs`).
- [ ] Audit and triage `unwrap/expect` in Tier 1 (production) paths.
- [ ] Audit and triage `println/eprintln/dbg` in Tier 1 paths.
- [ ] Optional: label experimental crates (e.g. `locald-vmm`) clearly in docs and/or crate-level docs.

## 5. Context Updates (Stage 3)

- [ ] Update `docs/manual/` with a “Contributing / Validation” page that mirrors CI.
- [ ] Update `docs/agent-context/plan-outline.md` to record completion of this cleanup pass.

## 6. Drawbacks

- This work is low-glamour and can consume time if we chase perfection.
- Tightening lint policies may require small refactors.

## 7. Alternatives

- Do nothing and rely on reviewers to infer the right commands and project structure.
- Only add a root README and skip deeper consistency work.

## 8. Unresolved Questions

1. Should `scripts/check` be CI-equivalent, or should we make CI-equivalence explicit via a separate `scripts/ci-local.sh` wrapper?
2. Should `locald-vmm` be explicitly labeled experimental (or feature-gated) to avoid confusing “production expectations”?

## 9. Validation (CI Equivalence)

These checks should remain aligned with `.github/workflows/ci.yml`.

Rust:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `./scripts/ci-rust-checks-local.sh` (fast mode)
- `LOCALD_PREPUSH_FULL=1 ./scripts/ci-rust-checks-local.sh` (CI-like mode; includes sudo + e2e)

Web:

- `pnpm -C locald-dashboard install --frozen-lockfile && pnpm -C locald-dashboard build`
- `pnpm -C locald-docs install --frozen-lockfile && pnpm -C locald-docs build`
