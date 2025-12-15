---
title: Testing Strategy
---

# Testing Strategy

`locald` employs a multi-layered testing strategy to ensure correctness, stability, and a high-quality user experience.

## Philosophy: No Mocks, Just Fakes

We adopt the "No Mocks, Just Fakes" philosophy (RFC 0082).

- **No Mocks**: We avoid mocking libraries that hijack function calls at runtime.
- **Fakes**: We implement "Fake" versions of core dependencies (FileSystem, Network, ProcessRunner) that behave like the real thing but run in memory.
- **Dependency Injection**: Components accept these dependencies via Traits, allowing us to inject Fakes during tests.

## Test Layers

### 1. Unit & Integration Tests

- **Scope**: Individual functions, structs, and component interactions.
- **Location**: `src/` (unit) and `tests/` (integration) within each crate.
- **Dependencies**: Uses Fakes for speed and determinism.
- **Goal**: Verify logic, edge cases, and state transitions.

### 2. End-to-End (E2E) Tests (`locald-e2e`)

- **Scope**: The full `locald` binary and CLI.
- **Location**: `locald-e2e/` crate.
- **Dependencies**: Real binary, Real OS (Sandboxed).
- **Goal**: Verify the binary boots, binds ports, handles commands, and manages processes correctly in a real environment.
- **Sandboxing**: Uses `--sandbox <NAME>` to isolate the test environment (filesystem, sockets) from the user's actual configuration.

### 3. Dashboard E2E Tests (`locald-dashboard-e2e`)

- **Scope**: The Web Dashboard (Frontend) + Real Backend.
- **Location**: `locald-dashboard-e2e/` (Playwright).
- **Goal**: Verify the "Glass Box" nature of the dashboard—that the UI accurately reflects and controls the backend state.

#### The "Glass Box" Strategy (RFC 0083)

We verify the dashboard by asserting on the **Backend State**, not just the UI pixels.

- **Hybrid Assertion**:

  1.  **Action**: Click "Start" in the UI.
  2.  **UI Assertion**: Expect status badge to turn Green.
  3.  **Backend Assertion**: Run `locald status` CLI and assert the service is running.

- **Anti-Flake Measures**:
  - **Animation Freeze**: CSS animations are disabled during tests.
  - **Text Assertions**: We assert on terminal text content, not screenshots.
  - **Trace Artifacts**: Full traces (DOM, Network, Console) are captured on failure for AI debugging.

## Systematic Investigation Protocol

When debugging failures, we follow the **Systematic Investigation Protocol** (RFC 0066):

1.  **Observation**: State raw facts (Command, Output, Context).
2.  **Source of Truth**: Consult specs (OCI, CNB) before guessing.
3.  **The Split**: Design a test to rule out half the search space (e.g., "Is it the Shim or the Daemon?").
4.  **Hypothesis**: Formulate and falsify specific hypotheses.
5.  **Execution**: Run tests in order.

This prevents "mashing" and ensures we find the root cause efficiently.
