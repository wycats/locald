---
title: "Premium Dashboard Experience"
stage: 3 # Recommended
feature: Dashboard
---

# RFC 0081: Premium Dashboard Experience

## 1. Summary

This RFC outlines the design and architecture for the "Phase 81" dashboard refinement. The goal is to transition `locald` from a simple process manager to a "Premium" local development environment. This involves a shift in mental model from "flat services" to "Projects", rich visualization of state and dependencies, and a sophisticated log interaction model.

## 2. Prior Art Analysis

To define "Premium", we look at industry leaders in developer experience:

- **Vercel**:
  - _Key Insight_: **Project-Centricity**. Users don't just run "scripts"; they develop "Projects". The dashboard should reflect this hierarchy.
  - _Takeaway_: Group services under a "Project" header (defined by the `locald.toml` location).
- **Railway**:
  - _Key Insight_: **Visualizing Relationships**. The "Canvas" view makes dependencies (Service A needs Redis) intuitive.
  - _Takeaway_: While a full canvas might be overkill for now, we must visually indicate dependencies (e.g., "Waiting for Database").
- **Tilt**:
  - _Key Insight_: **Smart Logs**. Tilt collapses successful build steps and only expands failures. This reduces cognitive load significantly.
  - _Takeaway_: Implement "Smart Folding" for logs.
- **TurboRepo**:
  - _Key Insight_: **Task Graph**. Visualizing what is running and why.
  - _Takeaway_: Explicitly show the state of ephemeral tasks (`locald run`).

## 3. UI Architecture

### 3.1. Navigation Structure (The "Shell")

The application shell will move from a simple top-bar to a **Sidebar Layout** to accommodate the "Project" hierarchy.

- **Sidebar (Left Pane)**:
  - **Context Switcher**: Dropdown to switch between active `locald.toml` environments (if multiple are supported in the future) or "Workspaces".
  - **Project Tree**:
    - **App Services**: The core long-running services defined in `locald.toml`.
    - **Tasks**: Ephemeral or one-off scripts (e.g., `db:migrate`).
    - **System**: `locald` daemon status and global logs.
  - **Status Summary**: A mini-graph or traffic light system showing overall project health at a glance.

### 3.2. Project View (The "Home")

The main view is no longer just a list. It is a **Dashboard**.

- **Header**:
  - Project Name (derived from directory or `locald.toml`).
  - **Global Controls**: "Start All", "Stop All", "Restart All".
  - **Environment Badges**: e.g., `NODE_ENV=development`.
- **Service Grid**:
  - Services are represented as **Cards** rather than table rows.
  - **Grouping**: Services can be visually grouped (e.g., "Backend", "Frontend", "Infrastructure") if tagged in `locald.toml`.

### 3.3. Service Representation (The "Card")

Each service card is a high-density information display:

- **Header**: Service Name + Status Icon + Port Link (e.g., `localhost:3000` ↗).
- **Body**:
  - **Mini-Log**: The last 3 lines of output (faded).
  - **Metrics**: (Future) Sparkline for CPU/RAM.
- **Footer**:
  - **Actions**: Start/Stop toggle, Restart button, "Open Terminal" button.
  - **Config Toggle**: A button to slide out the **Inspector Drawer**.

### 3.4. The Inspector Drawer (Configuration Visibility)

Clicking a service opens a drawer on the right side (overlaying the content), keeping context.

- **Tabs**:
  - **Overview**: Full status, PID, uptime.
  - **Config**:
    - **Command**: The exact command being run.
    - **Env Vars**: A key-value table. Values are masked (`*****`) by default with a "Reveal" eye icon.
    - **Working Dir**: Path to the service root.
  - **Dependencies**: List of services this service depends on, and their current status.

### 3.5. Log Interface (Smart Folding & Event Stream)

The log view is the primary workspace for developers.

- **The "News Feed" (Project Level)**:
  - A unified stream showing high-level events from _all_ services.
  - _Example_: `[10:00:01] [postgres] Started on port 5432`, `[10:00:02] [api] Connected to DB`.
- **Service Logs (Drill-down)**:
  - **Smart Folding**: Blocks of logs (detected by heuristics or markers) are collapsed if successful.
    - _Example_: `> npm install ... (Done in 2s)` [Collapsed].
    - _Failure_: If a block fails, it auto-expands.
  - **Search & Filter**: Sticky search bar to filter by regex.

## 4. Specific Solutions

### 4.1. Status Visual Language

We will use a combination of **Color**, **Icon**, and **Animation** to convey state without ambiguity.

| State        | Color  | Icon            | Animation | Text                  |
| :----------- | :----- | :-------------- | :-------- | :-------------------- |
| **Running**  | Green  | `CheckCircle`   | None      | "Healthy" / "Running" |
| **Starting** | Blue   | `Loader`        | Spin      | "Starting..."         |
| **Building** | Purple | `Package`       | Pulse     | "Building..."         |
| **Stopped**  | Grey   | `StopCircle`    | None      | "Stopped"             |
| **Error**    | Red    | `AlertTriangle` | None      | "Crashed (Exit 1)"    |
| **Waiting**  | Yellow | `Clock`         | Pulse     | "Waiting for [db]..." |

### 4.2. Dependency Visualization

To avoid a messy graph, we use **Contextual Highlighting**.

- **In the Grid**: If Service A depends on Service B:
  - Service A's card shows a small "Link" icon with "Waiting for B" if B is not ready.
  - Hovering Service A highlights Service B in the grid.
- **Enforcement**: The "Start" button on Service A is disabled (or shows a warning) if Service B is stopped, with a tooltip "Requires 'postgres'".

### 4.3. Ephemeral Tasks

One-off commands (`locald run test`) need a home.

- **The "Task Runner"**: A section in the Sidebar.
- **Behavior**:
  - When a task is run, it spawns a **Temporary Card** at the top of the Service Grid.
  - This card shows the command status and streams logs.
  - Once finished, the card remains (in a "Completed" state) until dismissed, allowing the user to inspect the result.
  - **"Rerun"**: A quick action on the completed card to run it again.

## 5. Implementation Strategy

1.  **Design System Update**: Implement the Status Icons and Card components in Svelte.
2.  **Data Model**: Update the frontend store to group services by Project and handle "Ephemeral" service types.
3.  **Log Logic**: Implement the "Smart Folding" logic in the frontend log viewer component.
4.  **Layout**: Refactor the main page into `Sidebar` + `Grid` layout.
