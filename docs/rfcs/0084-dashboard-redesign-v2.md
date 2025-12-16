---
title: "Dashboard Redesign v2: The Immersive Workspace"
stage: 0 # Strawman
feature: Design
---

# RFC 0084: Dashboard Redesign v2 - The Immersive Workspace

## 1. Summary

This RFC proposes a significant redesign of the Dashboard's "Inspector" and "Sidebar" components to align with **Axiom 2 (The Dashboard is a Workspace)**. It moves away from the "Drawer" metaphor towards an "Immersive View" for logs, restores quick actions to the sidebar, and enforces the use of stable domains over ephemeral ports.

## 2. Motivation

### 2.1. The "Keyhole" Problem

The current Inspector implementation (a side drawer) treats the terminal as a secondary metadata field. This forces developers to view logs through a "keyhole"—a small, scrolling box inside another scrolling panel. This violates **Axiom 2**, which states the dashboard should feel as capable as a local terminal. When a developer opens the Inspector, they are usually there to _debug_, which requires maximum screen real estate.

### 2.2. The "Passive" Sidebar

The current sidebar is merely a navigation aid. To restart a service, a user must find the card in the grid or open the inspector. This adds friction to common workflows (e.g., "Restart the backend while I work on the frontend").

### 2.3. The "Port" Leak

Displaying `localhost:43215` on cards exposes implementation details (ephemeral ports) that `locald` is designed to abstract away. It trains users to rely on unstable ports rather than the stable `http://<service>.localhost` domains provided by the proxy.

## 3. Proposal

### 3.1. The Immersive Inspector (The "Stage")

We will replace the "Drawer" with a **Focus Mode** that transforms the entire dashboard.

- **Layout**:
  - **Header (Compact)**: Contains the Service Name, Status Indicator, and Primary Actions. Height: ~50px.
  - **Main Body (The Terminal)**: Takes up the remaining height. No nested scrolling.
  - **The "Rail" (Right Side)**: A collapsed strip of metadata icons (Config, Env, Ports). Hovering expands them. This keeps the terminal width maximal.
- **Functional Delight**:
  - **Clickable Stack Traces**: The terminal parser will detect file paths (e.g., `src/main.rs:42`) and render them as links that open the file in the user's configured editor (VS Code).
  - **"Live Grep"**: A search bar in the header that filters the log stream in real-time, highlighting matches and hiding non-matching lines (with a "show context" toggle).
  - **Input Injection**: A distinct input field at the bottom to send `stdin` to the process, enabling interactive debugging (e.g., `pdb`, `binding.pry`).

### 3.2. Sidebar 2.0: The Command Center

The sidebar will become an active control surface, optimized for keyboard users.

- **Keyboard Navigation**: `j`/`k` to traverse the list. `Enter` to focus. `Esc` to return to grid. `r` to restart the highlighted service.
- **Labeling**: Display the **Domain Name** (e.g., `web`) by default.
- **Quick Actions**:
  - On hover (or selection), show icon buttons: `[Restart]`, `[Open]`, `[Terminal]`.
- **"Pulse" Status**: When a service emits a log line, its status dot briefly pulses, giving a peripheral sense of activity without noise.

### 3.3. Domain-First Presentation

- **Cards**: The "Link" area on the Service Card must prioritize the `locald` proxy domain (e.g., `http://web.dotlocal.localhost`).
- **Copying**: Clicking the link copies the stable domain.

### 3.4. The "Omnibar" (Future)

A global command palette (`Cmd+K`) to jump to any service, run `locald` commands, or toggle settings, bypassing the mouse entirely.

## 4. Design Mockup Description

**The Focus Mode:**

```text
+---------------------------------------------------------------+
| [Sidebar] |  docs-dev  (● Running)   [Search Logs...] [X]     | <- Header
|           |---------------------------------------------------|
| (Dimmed)  | 16:58:44 watching for file changes...             | [Config]
|           | 16:58:45 [HMR] connected                          | [Env]
|           | Error at src/lib/api.ts:20:5  <-- Clickable       | [Ports]
|           |                                                   |
|           |                                                   | <- Rail
|           |                                                   |
|           | > Send input...                                   |
+---------------------------------------------------------------+
```

**The New Sidebar Item:**

```text
[● web.localhost      [R] [T] [↗]]
```

## 5. Implementation Plan

1.  **Refactor Sidebar**: Update `Sidebar.svelte` for keyboard nav and actions.
2.  **Refactor ServiceCard**: Update URL display logic.
3.  **Rebuild Inspector**:
    - Create `InspectorFocus.svelte`.
    - Implement `xterm.js` with `webgl` addon for performance.
    - Implement "LinkProvider" for `xterm.js` to handle file paths.
    - Implement `stdin` piping via the websocket.

## 6. Axiom Alignment

- **Axiom 1 (Zero-Friction)**: Keyboard nav and editor links remove friction from the "Edit-Debug" loop.
- **Axiom 2 (Workspace)**: Interactive input and live grep make it a true workspace.
- **Axiom 9 (Managed Networking)**: Emphasizing `*.localhost`.
