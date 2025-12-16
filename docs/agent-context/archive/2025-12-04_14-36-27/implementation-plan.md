# Implementation Plan - Phase 24: Dashboard Ergonomics & Navigation

**Goal**: Transform the Dashboard from a passive "Monitor" into an active "Workspace" where you can control your environment and navigate context efficiently.

## 1. Global Workspace Controls

- [ ] **Connection Status**:
  - Add a prominent indicator in the header showing the connection state to the `locald` daemon.
  - Handle "Connecting", "Connected", and "Disconnected" states gracefully.
  - Show a "Reconnecting..." overlay or banner when the connection is lost.
- [ ] **Global Actions**:
  - Implement a "Restart All" button in the header.
  - Implement a "Stop All" button in the header.
  - Wire these up to new API endpoints in `locald-server`.

## 2. The Event Stream (Solving the "All Services" Chaos)

- [ ] **Backend**:
  - Create a new `LogEntry` type that includes metadata (service name, timestamp, event type).
  - Implement a "Lifecycle Event" stream that emits events like `ServiceStarted`, `ServiceStopped`, `HealthCheckFailed`.
- [ ] **Frontend**:
  - Replace the raw `xterm.js` view for "All Services" with a structured list or a specialized terminal view.
  - **Sanitization**: Strip cursor movement codes from logs in this view to prevent overwriting.
  - **Interleaving**: Merge log lines and lifecycle events into a single chronological stream.
  - **Attribution**: Clearly prefix each line with the service name (e.g., `[api]`, `[db]`) with distinct colors.

## 3. Navigation & Deep Linking

- [ ] **URL Sync**:
  - Update the browser URL when a service is selected (e.g., `?service=web`).
  - Ensure reloading the page restores the selected service.
- [ ] **History Management**:
  - Ensure the browser's Back/Forward buttons work correctly to navigate between services.

## 4. Visual Polish

- [ ] **Sidebar**:
  - Add "Traffic Light" status indicators (Green=Running, Red=Error, Grey=Stopped, Yellow=Starting) next to service names.
  - Group services (preparatory work for Constellations).
- [ ] **Service Details**:
  - Make `.localhost` URLs clickable links.
  - Clean up the layout of the service details pane (PID, Port, Config).
