# Dashboard Surface Inventory (Phase 112)

This is the Phase 112 inventory of *user-visible* dashboard concepts: labels, navigation terms, and actions.

Goal: keep dashboard vocabulary aligned with the Surface Contract program (RFC 0114) and avoid collisions with other surfaces (notably: reserve “pin” for registry policy; use “monitor” for dashboard focus).

## Core nouns

- **Workspace**: the set of projects/services currently visible in the dashboard.
- **Project**: a group of services shown together.
- **Service**: a runnable unit with status/logs and optional URL.
- **System**: system projects (names starting with `locald-`) shown under a “System” section.

## Core surfaces

- **Rack** (sidebar): project/service list + per-service controls.
  - User-visible terms:
    - “System” (section header)
    - “System Normal” (footer entry)
    - Service controls: “Start”, “Stop”, “Restart”, “More”, “Open”, “Terminal” (tooltips)
    - Deck focus control: **“Monitor in Deck”** (tooltip)

- **Stream** (main view default): unified log stream.

- **Deck** (main view when monitoring): focused tiled layout for one or more services.
  - User-visible terms:
    - “Daemon Control Center” (when monitoring `locald`)
    - Deck removal action: **“Stop monitoring”**

- **Daemon Control Center** (inside the Deck for `locald`): daemon status + recent activity.
  - User-visible terms:
    - “Connection”, “Connected/Disconnected/Unknown”
    - “Workspace Summary”, “Projects”, “Services”, “Running”, “Building”, “Stopped”

## Canonical dashboard vocabulary decisions

- **Deck focus uses “monitor”** (not “pin”).
  - Rationale: “pin” is reserved for registry policy (retain/autostart), and using it in the dashboard creates a vocabulary collision.

## Docs pages that teach the dashboard model

- `locald-docs/src/content/docs/getting-started/index.mdx`
- `locald-docs/src/content/docs/concepts/workspace.md`
