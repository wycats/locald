# Design: Dashboard Ergonomics (Phase 24)

## The "All Services" Problem

**Question**: Does an "All Services" log view make sense if services are running in PTYs?

**Analysis**:

- **Technical Conflict**: PTYs are stateful (cursor positioning, clearing lines). Interleaving two PTY streams into a single view is technically incoherent. If Service A moves the cursor up 2 lines to redraw a progress bar, but Service B has printed 5 lines in the meantime, the output will be garbled.
- **User Intent**: When users look at "All Services", they are usually looking for **correlation** (e.g., "Service A failed right after Service B started") or **activity** (e.g., "Is anything happening?").

**Solution: The Event Stream**
Instead of a raw multiplexed PTY stream, the "All Services" view should be a **Structured Event Stream**:

1.  **Sanitized Output**: Strip complex ANSI control codes (cursor movement), keeping only basic formatting (colors).
2.  **Lifecycle Events**: Interleave process events ("Service Started", "Service Crashed", "Health Check Failed") with the log lines.
3.  **Source Attribution**: Clearly label each line with its source service (e.g., `[api] GET / 200`, `[db] Connection accepted`).

## Global Status & Control Header

**Goal**: Elevate the dashboard from a passive "monitor" to an active "workspace controller".

**Proposed UI Elements**:

1.  **Daemon Connection Status**:
    - Clearly indicate if the dashboard has lost connection to the `locald` daemon.
    - Distinguish between "Daemon Down" and "Network Error".
2.  **Global Actions**:
    - **"Restart All"**: A panic button to reset the entire environment.
    - **"Stop All"**: Quickly quiet the machine.
3.  **Platform Health**:
    - **Update Available**: If a new version of `locald` is detected.
    - **System Load**: CPU/Memory usage of the constellation.

## Refined Layout

- **Sidebar**: Service list with "traffic light" status indicators.
- **Main Area**:
  - **Default**: The "Constellation" view (Event Stream + Topology).
  - **Focused**: Single service PTY (fully interactive).
