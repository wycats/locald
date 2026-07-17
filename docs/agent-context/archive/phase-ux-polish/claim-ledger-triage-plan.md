# Claim Ledger Triage Plan

## 1) Summary

This plan triages the existing **User Programming Model audit** claim ledger into an execution schedule.

Decision checkpoint (triage): **Option A approved** (bucket model + priorities) on **2025-12-16**.

- **Target boundary:** `dotlocal` repo
- **Primary surface:** `locald` user-facing CLI + user-facing documentation (docs site + manual)
- **Audit artifact:** [docs/rfcs/stage-0/0112-user-programming-model-audit-and-doc-plan.md](../../rfcs/stage-0/0112-user-programming-model-audit-and-doc-plan.md)
- **Claim ledger location:** Claim Ledger table in the audit artifact (CL-001 … CL-061)
- **Conflict cards / decision logs:** “Conflicts Requiring Decision (STOP List)” in the audit artifact (C-001 … C-013)

Top 3 priority themes (recommendation):

1. **Front door correctness**: eliminate “unknown command” and “copy/paste fails” in Getting Started + CLI Reference + `locald init` output.
2. **Single canonical vocabulary**: converge docs and CLI on one set of verbs (up/monitor/dashboard/registry clean/run/try).
3. **Privilege boundary truth**: align docs with the shim-based privileged model (stop teaching setcap / `locald-shim server start` / stable `~/.cargo/bin` paths).

## 2) Triage Scope & Assumptions

### Target boundary + primary surface

- **Primary surface (in-scope):**
  - `locald` CLI command surface, help text, and interactive prompts
  - user-facing docs: `README.md`, `locald-docs/` pages, and `docs/manual/`
- **Secondary surfaces (in-scope only as they affect the primary):**
  - daemon lifecycle controls (`locald server start|shutdown`) when taught in user docs
  - dashboard terminology when it collides with CLI terminology

### In-scope

- Claim ledger rows CL-001 … CL-061 that describe user-facing contracts.
- Doc mismatches that cause broken flows or misleading promises.

### Out-of-scope (for this triage)

- Large feature development not already implied by the claim ledger (e.g. implementing Mark‑Sweep GC end-to-end). These can be scheduled as follow-ups, but are not expanded into detailed specs here.
- Rewriting RFC “law” beyond noting that a follow-up RFC/manual alignment task is needed.

### Assumptions that affect prioritization

- **Privileged setup is assumed for now**: canonical remediation is `locald doctor` → `sudo locald admin setup`.
- **Strict single-canon vocabulary** is a goal: docs should teach one spelling, with migration hints allowed but not multiple “real” commands.
- **Triage artifact is planning-only**: no changes are performed here.

## 3) Ledger Normalization Notes

- The claim ledger already has the key columns needed for triage (ID, claim, status, persona impact, evidence).
- “Conflict Cards” exist inside the audit artifact; in this plan they become **Bucket C** items only if they still require an explicit user decision.
- Some ledger rows overlap and will later be grouped into “themes” for scheduling (e.g. vocabulary drift across multiple pages). This plan will keep the original IDs but may schedule them together.

## 4) Scoring Model (Coarse, Human-Readable)

Scales (Low/Med/High):

- **Severity**: does this break the golden path / cause immediate failure?
- **Blast radius**: how many personas/workflows are affected?
- **Leverage**: does fixing it collapse multiple other items?
- **Effort**: docs-only vs code change vs cross-surface refactor
- **Evidence quality**: Strong/Weak (affects confidence)

Priority rule:

- **Priority = (Severity + Blast + Leverage) − Effort**, then adjust down if **Evidence quality is Weak**.

## 5) Triage Buckets (The Four-Way Split)

### A — Docs-only, safe

| ID              | Claim / Issue                                                            | Status    | Persona impact           | Contract impact                 | Evidence quality | Proposed action                                                                                                         | Notes                                     |
| --------------- | ------------------------------------------------------------------------ | --------- | ------------------------ | ------------------------------- | ---------------- | ----------------------------------------------------------------------------------------------------------------------- | ----------------------------------------- |
| CL-013 / CL-050 | Docs teach `locald down`, but CLI doesn’t implement it                   | Drift     | App Builder              | Broken command                  | Strong           | Remove `down` from user-facing docs until implemented                                                                   | Conflict Card C-006 already proposes this |
| CL-014          | CLI reference docs omit major commands / include wrong ones              | Drift     | App Builder / Power User | Broken discovery + bad guidance | Strong           | Update CLI reference to match current CLI surface                                                                       | Should be treated as “front door”         |
| CL-054          | Getting started teaches `locald run <cmd>` but that mode doesn’t exist   | Drift     | App Builder              | Copy/paste fails                | Strong           | Update Getting Started to use `locald try ...`                                                                          | Closely tied to `try` contract            |
| CL-055          | CI docs teach `kill $PID` teardown for `locald up`                       | Drift     | CI users                 | Unreliable teardown             | Strong           | Update CI docs to use `locald server shutdown` (sandboxed)                                                              | Avoid conflating CLI client vs daemon     |
| CL-048          | Docs teach `locald-shim server start` but shim has no such command       | Drift     | Operator                 | Misleads privileged model       | Strong           | Rewrite docs: shim is privileged helper; daemon starts via `locald`                                                     | Aligns with leaf-node axiom               |
| CL-047          | Docs teach `sudo locald admin sync-hosts` as required setup “per domain” | Ambiguous | App Builder              | Mis-states the contract         | Strong           | Teach: run `sudo locald admin setup` once; `locald up` auto-syncs hosts; if manual sync-hosts is needed, treat as a bug | Decision recorded below                   |

### B — Implementation, safe

| ID     | Claim / Issue                                                          | Status | Persona impact | Contract impact             | Evidence quality | Proposed action                                                                                                                                   | Notes                                  |
| ------ | ---------------------------------------------------------------------- | ------ | -------------- | --------------------------- | ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------- |
| CL-012 | `locald init` instructs `locald start`                                 | Drift  | App Builder    | Immediate “unknown command” | Strong           | Change init success message to canonical next step (`locald up`)                                                                                  | Matches Conflict Card C-005 resolution |
| CL-008 | `locald init` defaults domain to `.local`                              | Drift  | App Builder    | Mixed domain defaults       | Strong           | Change init default to `.localhost` (or empty)                                                                                                    | Matches Conflict Card C-003 resolution |
| CL-034 | `ai context` docs promise rich Markdown dump; impl returns JSON status | Drift  | Integrators    | Overpromised contract       | Strong           | Implement staged contract: default **Markdown** output with an explicit `--json` mode; phase in richer context (logs/config/doctor signals) later | Decision recorded below                |

### C — Needs decision

| ID  | Claim / Issue | Status | Persona impact | Contract impact | Evidence quality | Proposed action | Notes |
| --- | ------------- | ------ | -------------- | --------------- | ---------------- | --------------- | ----- |

_(empty)_

### D — Defer

| ID     | Claim / Issue                                       | Status | Persona impact | Contract impact         | Evidence quality | Proposed action                                              | Notes                                          |
| ------ | --------------------------------------------------- | ------ | -------------- | ----------------------- | ---------------- | ------------------------------------------------------------ | ---------------------------------------------- |
| CL-024 | Mark‑Sweep GC described as current; impl is partial | Drift  | Operator       | Misaligned expectations | Strong           | Treat GC as planned; schedule later implementation milestone | Conflict Card C-009 already resolves “planned” |

## 6) Decision Queue (Handle C Items Turn-Based)

Resolved decisions:

- **2025-12-17 — CL-047:** hosts syncing is part of the core “it just works” contract once privileged setup is complete. Docs should not frame `sync-hosts` as a repeated/manual step; needing `locald admin sync-hosts` after `locald up` is considered a bug.
- **2025-12-17 — CL-034:** implement the “AI-friendly” contract rather than downgrading docs. Stage it: default output becomes **Markdown** (LLM-optimized) with an explicit `--json` option for machine consumption; add richer context (e.g. unhealthy logs/config excerpts/doctor signals) as a follow-up phase.

| ID  | Decision needed | Options | Recommendation | Why now |
| --- | --------------- | ------- | -------------- | ------- |

_(none)_

STOP rule for triage sessions: resolve one decision at a time, update this section, then proceed.

## 7) Epoch Schedule (Execution Plan)

### Epoch T1 — Front Door Truth

- **Goal:** remove copy/paste failures and unknown-command loops on the golden path.
- **Inputs:** CL-012, CL-013/CL-050, CL-014, CL-054.
- **Deliverables:**
  - Docs updates: Getting Started + CLI Reference reflect real CLI.
  - CLI tweak: `locald init` message uses canonical next step.
- **Verification gates:** `pnpm -C locald-docs build`, `cargo test` (workspace), doc link checkers if available.
- **Exit criteria:** a new user can follow Getting Started without hitting a non-existent command.

### Epoch T1.5 — AI Usability (High-Leverage Debugging)

- **Goal:** make “one command to capture context” real for AI agents and debugging.
- **Inputs:** CL-034.
- **Deliverables:**
  - `locald ai context` default output is **Markdown** (LLM-optimized).
  - Add explicit `--json` (or equivalent) for stable machine consumption.
  - Keep the initial scope minimal (status summary + embedded JSON is acceptable); schedule “logs/config/doctor signals” as a follow-up enhancement.
- **Verification gates:** `cargo test` (workspace); smoke-run `locald ai context` against a running daemon.
- **Exit criteria:** docs and CLI agree on the contract, and the default output is human/LLM readable without extra flags.

### Epoch T2 — Domain & Vocabulary Canon

- **Goal:** make `.localhost` and vocabulary single-canon across surfaces.
- **Inputs:** CL-008, CL-026, CL-027, CL-028/CL-052/CL-053 (as follow-on once decisions are confirmed).
- **Deliverables:** doc sweep + small CLI adjustments where needed.
- **Verification gates:** doc build + grep-based assertions (optional) + CLI help snapshots (if adopted).
- **Exit criteria:** docs teach one vocabulary set; `locald init` doesn’t introduce legacy `.local`.

### Epoch T3 — Privilege Boundary Truth

- **Goal:** align docs with shim reality (FD passing, sibling install/discovery).
- **Inputs:** CL-042, CL-046, CL-048, CL-056, CL-057, CL-058, CL-059, CL-060, CL-061.
- **Deliverables:** doc corrections; optional implementation only if we choose to support env/PATH discovery.
- **Verification gates:** doc build; `locald doctor` outputs match taught remediation.
- **Exit criteria:** no docs page teaches setcap or `locald-shim server start`.

## 8) Change Packaging (Optional but Recommended)

- Docs-only PRs first (Epoch T1 docs portions; then T2 docs sweep; then T3 docs).
- Small CLI PRs next (`locald init` message + domain default).
- Broader implementation PRs last (e.g. flipping primary `run`/`exec` surface, richer AI context).

## 9) Risks & Circuit Breakers

- Risk: changing canonical verbs may disrupt muscle memory; mitigate with “did you mean …” messaging rather than aliases.
- Risk: promising stable paths (“~/.cargo/bin”) harms non-cargo installs; prefer sibling-of-exe contract.
- Circuit breaker: if we discover multiple docs authorities disagree (manual vs docs site vs RFC), pause and re-triage the authority model for that topic.

## 10) Acceptance Criteria

Done when:

- Every CL row is bucketed A/B/C/D.
- Decision queue exists and is ordered.
- Multi-epoch schedule exists with verification gates and exit criteria.
- User approves the schedule as input for remediation epochs.
