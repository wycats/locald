---
title: "Dashboard Self-Representation & Real-time Health"
stage: 1 # Accepted
feature: Dashboard
---

# RFC 0073: Dashboard Self-Representation & Real-time Health

## 1. Summary

This RFC formalizes the behavior of the `locald` dashboard regarding its own representation within the service list, and mandates real-time updates for service health status.

## 2. Motivation

### 2.1. The "Dashboard Paradox"

The dashboard is itself a service running within `locald` (typically `dotlocal:web`). However, treating it exactly like any other user service leads to UX issues:

- **Confusion**: It appears as a "Starting" or "Unhealthy" service if it doesn't have a standard health check.
- **Risk**: Users might accidentally "Stop" or "Restart" the dashboard they are currently using, severing their connection.
- **Redundancy**: Displaying a `localhost` URL for the dashboard is redundant since the user is already viewing it.

### 2.2. The "Stale State" Problem

Users expect the dashboard to reflect the _current_ state of the system. If a service becomes healthy, the UI should update immediately. Relying on page refreshes or slow polling makes the workspace feel sluggish and untrustworthy.

## 3. Detailed Design

### 3.1. Dashboard Self-Awareness

The dashboard frontend must identify the service entry corresponding to itself.

- **Identifier**: `dotlocal:web` (or a configured system service name).
- **Visual Distinction**:
  - **Status Indicator**: Use a distinct color (e.g., Blue) to signify "System Service" or "This Dashboard".
  - **Controls**: Hide "Start", "Stop", and "Restart" controls to prevent accidental self-termination.
  - **Metadata**: Hide redundant information like the URL.

### 3.2. Real-time Health Broadcasting

The `locald-server` must broadcast health state changes immediately.

- **Mechanism**: Server-Sent Events (SSE) or WebSockets.
- **Event Trigger**: The `HealthMonitor` must emit a `ServiceUpdate` event the moment a probe succeeds or fails.
- **Frontend Reaction**: The dashboard must listen for these events and update the local state store without a full refresh.

### 3.3. Service Status Logic

The logic for determining a service's status (Running, Stopped, Healthy, etc.) should be consistent between the backend API and the frontend display.

- **Backend**: `Service::to_status` should encapsulate the logic for deriving the public status from internal runtime state.

## 4. Implementation Plan

1.  **Backend**:
    - Refactor `Service::to_status` in `locald-server` for reusability.
    - Update `HealthMonitor` to emit `ServiceUpdate` events upon health changes.
2.  **Frontend**:
    - Update `+page.svelte` to special-case `dotlocal:web`.
    - Implement distinct styling for the dashboard service.
    - Ensure the `EventSource` listener correctly handles `ServiceUpdate` events.

## 5. Drawbacks

- **Special Casing**: Hardcoding `dotlocal:web` in the frontend couples the UI to a specific service name.
  - _Mitigation_: The backend could send a flag `is_dashboard: true` in the service payload, making it dynamic. (Future improvement).

## 6. Alternatives

- **Hide it completely**: We could filter `dotlocal:web` out of the list entirely.
  - _Con_: Users might want to see logs or config for the dashboard itself.
- **Generic "System" Category**: Group all `dotlocal:*` services into a separate section.

## 7. Unresolved Questions

- Should we expose a "Restart Dashboard" button that triggers a graceful reload of the page after the server restarts?
