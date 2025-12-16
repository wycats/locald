---
title: Dashboard E2E Strategy
stage: 3
---

# RFC 0083: Dashboard E2E Strategy

## 1. Context

The `locald` dashboard is becoming a "Premium Workspace" (Axiom: The Dashboard is a Living Workspace). It is no longer just a read-only view; it has complex state, real-time updates (SSE), and interactive controls.

We need a testing strategy that verifies this interactivity without becoming flaky or slow, specifically tailored for an AI Agent workflow.

## 2. Philosophy: "The Glass Box"

Following [Axiom: The Dashboard is a Living Workspace](../design/axioms/experience/05-dashboard-philosophy.md), the dashboard must be transparent ("Glass Box"). Our tests should verify this transparency.

- **Verify Reality**: Tests should assert that what is shown on the screen matches the underlying system state.
- **Verify Interactivity**: Tests should assert that clicking a button actually changes the system state.

## 3. Strategy: Browser-Based E2E

We will use **Playwright** to test the dashboard running against a real (sandboxed) `locald` instance.

### 3.1. Why Playwright?

- **Real Browser**: It tests the actual rendering engine (Chrome/WebKit/Firefox), catching CSS/JS issues.
- **Resilience**: It has built-in auto-waiting and retry logic, reducing flakiness.
- **Tracing**: It provides full traces (screenshots, DOM snapshots) for debugging failures.

### 3.2. Agent-First Debugging

Since the primary debugger is an AI Agent, standard console logs are insufficient. We must configure Playwright to maximize artifact generation.

- **Mandatory Traces**: Enable `trace: 'retain-on-failure'` to capture DOM snapshots, network requests, and console logs.
- **Console Passthrough**: Pipe browser console logs to the test runner so the Agent can read frontend errors.

### 3.3. Anti-Flake Strategy

The dashboard features dynamic elements (xterm.js cursors, spinners) that cause flakiness. We will inject CSS to stabilize the UI.

- **Global Animation Freeze**: Pause all CSS animations and transitions.
- **Hide Cursors**: Force `caret-color: transparent` and `.xterm-cursor { visibility: hidden }`.
- **Text Assertions**: Avoid pixel-diffing terminals. Assert on text content buffers instead.

## 4. The Architecture

We will not mock the backend. We will run the **Real Frontend** against the **Real Backend**.

1.  **Global Setup**: The test harness spawns `locald` in a sandbox (using `locald-e2e` logic) via Playwright's `globalSetup`.
2.  **Health Check**: Wait for the daemon to respond to a health probe before starting tests.
3.  **Serve**: The test harness serves the dashboard (either via `vite preview` or the embedded server).
4.  **Drive**: Playwright drives the browser to interact with the dashboard.
5.  **Assert**: Playwright asserts on the UI state _and_ the backend state (via CLI/API).
6.  **Isolation**: Use a `reset_state` endpoint (debug builds only) or rely on sandbox cleanup.

## 5. The "Hybrid" Assertion

A key innovation in our strategy is **Hybrid Assertion**. We verify the UI by checking the Backend, and vice-versa.

**Example: Starting a Service**

1.  **Action**: Playwright clicks the "Start" button for `shop-backend`.
2.  **UI Assertion**: Expect the status badge to turn "Green".
3.  **Backend Assertion**: Run `locald status` (via CLI) and assert the service is actually running.

This ensures the dashboard isn't just "painting pixels" but is actually controlling the daemon.

## 6. Implementation Plan

### 6.1. `locald-dashboard-e2e`

Create a new directory `locald-dashboard-e2e` (or inside `locald-dashboard/e2e`).

- **Stack**: Playwright + TypeScript.
- **Harness**: A TypeScript wrapper around the `locald` binary (similar to `locald-e2e` but in Node.js).

### 6.2. Test Scenarios

1.  **Read-Only**:

    - Load dashboard.
    - Verify project list matches `locald.toml`.
    - Verify service config is visible (Inspector).

2.  **Interactive**:

    - Start/Stop service -> Verify status change.
    - Restart All -> Verify all services restart.

3.  **Real-Time**:

    - Trigger a log message (e.g., `curl` the service).
    - Verify the log appears in the dashboard terminal.

4.  **Network Resilience**:
    - Simulate SSE connection drops.
    - Verify "Reconnecting..." state and recovery.

## 7. Anti-Patterns (What NOT to do)

- **Do NOT Mock the API**: We want to verify the contract between Frontend and Backend.
- **Do NOT Test Implementation Details**: Don't assert on CSS classes (unless semantic). Use **Accessibility Concepts** (Roles, Labels, ARIA) if at all possible.
- **Do NOT use "Sleep"**: Use Playwright's `expect().toBeVisible()` which auto-waits.
- **Do NOT Pixel Diff Terminals**: Use text assertions on the terminal buffer.

## Appendix: Build Modes (Embedded vs Headless)

The embedded dashboard/docs server is part of the default `locald` build.

- **Default**: `locald` is compiled with the `ui` feature enabled and embeds static assets.
- **Headless**: `locald` is compiled with `--no-default-features` and does not embed UI assets. In this mode, E2E tests must serve the dashboard via `vite preview` (or another explicit frontend server), because the embedded server will intentionally return a "no UI" response.
