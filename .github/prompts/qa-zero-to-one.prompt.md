<!--
  QA: Zero to One (The Grand Simulation)
  Use this prompt to validate the system's robustness by simulating a fresh user journey.
-->

# QA: Zero to One (The Grand Simulation)

You are about to perform a "Zero to One" QA simulation. This process mimics a new user installing and using `locald` in a fresh environment to ensure the end-to-end flow is unbroken.

## The Goal

Validate that a user can go from "Zero" (no `locald`) to "One" (a fully managed, multi-service local development environment with SSL) without encountering friction, bugs, or confusing errors.

## The Personas

You will simulate the following personas during this process:

1.  **Riley (The App Builder)**: Wants to get their web app running quickly. Cares about "it just works" and seeing logs.
2.  **Sam (The Ops Engineer)**: Cares about correctness and security. Will check SSL certificates, process isolation, and graceful shutdown.
3.  **Alex (The Frontend Dev)**: Cares about the dashboard and feedback loops. Will check the Web UI and WebSocket updates.

## The Process

### 1. Setup the Simulation Environment

Create a clean, isolated directory for the simulation.

```bash
# 1. Build the latest binary
cargo build --release

# 2. Create the simulation directory
mkdir -p examples/grand-simulation
cd examples/grand-simulation

# 3. Ensure no existing daemon is running
locald server shutdown || true
```

### 2. The "RTFM" Challenge (Riley)

Instead of following hardcoded steps, you must act as a new user reading the documentation.

1.  **Locate the Docs**: Read the documentation source files in `locald-docs/src/content/docs/`. Start with the "Getting Started" or "Introduction" guide.
2.  **Follow Instructions**: Execute the commands exactly as they appear in the documentation.
3.  **Critique**: If a step is missing, unclear, or broken, document it immediately as a "Documentation Bug".

### 3. Verify the Result (Alex)

Once you have followed the guide, verify the expected outcome. The documentation should have led you to a state where:

1.  A project is initialized.
2.  At least one service is running.
3.  You can access the service via a `.localhost` domain.

*   **Verification**:
    *   Does the service URL load?
    *   Does `http://locald.localhost` (Dashboard) load?

### 4. Security & Robustness Check (Sam)

Inspect the underlying mechanics.

*   **SSL Check**:
    *   `curl -v https://web.localhost` -> Should be trusted (or at least served with a valid cert structure).
*   **Process Check**:
    *   `ps aux | grep python` -> Should see the processes.
    *   `locald stop web` -> Process should disappear.
*   **Config Persistence**:
    *   `locald server shutdown`
    *   `locald up` -> Should restore the previous state.

### 5. Report Findings

Document your findings in `docs/agent-context/qa-report.md`.

-   **Successes**: What worked well?
-   **Friction**: Where did you get stuck?
-   **Bugs**: Did anything fail?
-   **Axiom Violations**: Did the output violate any core design axioms?

## Checklist

- [ ] `locald init` creates a valid `locald.toml`.
- [ ] `locald up` starts the daemon and services.
- [ ] `*.localhost` domains resolve and route correctly.
- [ ] SSL certificates are generated and served.
- [ ] Dashboard is accessible at `locald.localhost`.
- [ ] Logs stream correctly via CLI and Dashboard.
- [ ] Graceful shutdown works (no zombie processes).
