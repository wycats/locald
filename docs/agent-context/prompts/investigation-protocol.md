# Systematic Investigation Protocol

**Goal**: Diagnose [Specific Error/Symptom] by systematically narrowing the search space.

## 1. The Observation
*State the raw facts. What happened? What was expected?*
- **Command**: `...`
- **Output**: `...`
- **Context**: (e.g., Sandbox, Setuid, CI)

## 2. The Search Space
*List the high-level domains where the fault could lie.*
1.  **Domain A** (e.g., The Binary/Tool itself)
2.  **Domain B** (e.g., The Configuration/Input)
3.  **Domain C** (e.g., The Environment/Permissions)
4.  **Domain D** (e.g., The Orchestrator/Shim)

## 3. The "Split" (Binary Search)
*What is the single most effective test to rule out half of the domains above?*
- **Test**: [Description of the isolation test]
- **Prediction A (Success)**: Implies the issue is in [Domain X].
- **Prediction B (Failure)**: Implies the issue is in [Domain Y].

## 4. Hypotheses & Falsification Plan
*List specific hypotheses and the test to prove/disprove them. Order by "Cheapest to Test" + "Highest Probability".*

| ID | Hypothesis | Test | Expected Result (if True) |
|----|------------|------|---------------------------|
| H1 | [e.g., Shim is not setuid] | `ls -l shim_path` | Permissions are not `-rws...` |
| H2 | [e.g., Config is invalid] | Run manually with `sudo` | Fails with same error |
| H3 | [e.g., Env vars missing] | Print env in shim | Missing `PATH` or `XDG_...` |

## 5. Execution
*Execute the tests in order. Stop and Analyze after "The Split".*
