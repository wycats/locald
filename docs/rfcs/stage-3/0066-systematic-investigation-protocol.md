---
title: Systematic Investigation Protocol
stage: 3
---

# RFC 0059: Systematic Investigation Protocol

## Context

When debugging complex failures in `locald`, especially those involving privileged operations (shim), containers (runc), or IPC, "mashing" (trying random fixes) is inefficient and risky. We need a structured approach to narrow down the search space scientifically.

## The Protocol

### 1. The Observation

_State the raw facts. What happened? What was expected?_

- **Command**: `...`
- **Output**: `...`
- **Context**: (e.g., Sandbox, Setuid, CI)

### 2. Source of Truth Review

_Before guessing, consult the authoritative specs._

- **Relevant Specs**: (e.g., OCI Runtime Spec, CNB Platform API)
- **Requirements**: Does the spec mandate specific Env Vars, Paths, or User IDs?
- **Deviation**: Is our implementation deviating from the spec?

### 3. The Search Space

_List the high-level domains where the fault could lie._

1.  **Domain A** (e.g., The Binary/Tool itself)
2.  **Domain B** (e.g., The Configuration/Input)
3.  **Domain C** (e.g., The Environment/Permissions)
4.  **Domain D** (e.g., The Orchestrator/Shim)

### 3. The "Split" (Binary Search)

_What is the single most effective test to rule out half of the domains above?_

- **Test**: [Description of the isolation test]
- **Prediction A (Success)**: Implies the issue is in [Domain X].
- **Prediction B (Failure)**: Implies the issue is in [Domain Y].

### 4. Hypotheses & Falsification Plan

_List specific hypotheses and the test to prove/disprove them. Order by "Cheapest to Test" + "Highest Probability"._

| ID  | Hypothesis                 | Test                     | Expected Result (if True)     |
| --- | -------------------------- | ------------------------ | ----------------------------- |
| H1  | [e.g., Shim is not setuid] | `ls -l shim_path`        | Permissions are not `-rws...` |
| H2  | [e.g., Config is invalid]  | Run manually with `sudo` | Fails with same error         |
| H3  | [e.g., Env vars missing]   | Print env in shim        | Missing `PATH` or `XDG_...`   |

### 5. Execution

_Execute the tests in order. Stop and Analyze after "The Split"._

### 6. Principles of Investigation

1.  **Instrument, Don't Guess**: If you can't see a variable (e.g., `EUID`, `PATH`, `Env Vars`), add code to print it. Assumptions about the environment are the most common source of "impossible" bugs.
2.  **Environment Fidelity**: Be aware of how your reproduction method (e.g., `cargo run`) differs from the production environment (e.g., `install-locald`).
3.  **Verify the Binary**: Ensure the binary running is the one you think it is (check paths, versions, permissions).

## Implementation Plan

- [ ] Adopt this protocol for the `runc` namespace investigation.
- [ ] Add to `docs/agent-context/prompts/` for future agent use.
