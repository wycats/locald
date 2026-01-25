---
title: "System Plane and Unified Pinning"
stage: 3 # Available
feature: Dashboard
---

# RFC 0100: System Plane and Unified Pinning

## 1. Summary

This RFC refines the "Cybernetic Workspace" (RFC 0087) by introducing a distinct **System Plane** for daemon observability and simplifying the interaction model by unifying **Solo Mode** and **Pinning** into a single "Pin" concept.

## 2. The Problem

### 2.1. The "Invisible Daemon"

The previous design treated `locald` (the daemon) as an invisible substrate. Users could see their project services, but had no visibility into the daemon's own logs, health, or resource usage. This made debugging the tool itself (or understanding why a service failed to start) difficult.

### 2.2. The "Solo vs. Pin" Confusion

RFC 0087 introduced two modes of viewing a service:

1.  **Solo Mode**: Click a row -> Exclusive view in "The Stream".
2.  **Pin Mode**: Click the monitor icon -> Persistent view in "The Deck".

This distinction proved unnecessary and confusing. "If only a single thing is monitored, it looks fine as a 'pin'". The cognitive load of managing two different "view states" outweighs the benefit of a quick "peek" mode.

## 3. The Solution

### 3.1. The System Plane

We introduce a **System Plane**—a dedicated observability layer for the `locald` daemon itself.

- **Entry Point**: A "System Normal" (or status) footer at the bottom of **The Rack**.
- **Interaction**: Clicking this footer toggles the **Daemon Control Center**.
- **Visualization**: The Daemon Control Center opens as a pinned card in **The Deck**, streaming the daemon's own internal logs (via `tracing`).
- **Identity**: The daemon is treated as a "virtual service" named `locald`.

### 3.2. Unified Pinning (Death of Solo)

We deprecate **Solo Mode**. The interaction model is simplified to a single **Pin** state.

- **Row Click**: Toggles the **Pin** state for that service.
- **Monitor Icon**: Redundant, but can remain as an explicit affordance for "Pinning".
- **Behavior**:
  - If 0 items are pinned: Show the global **Stream** (all logs).
  - If 1 item is pinned: Show that item in **The Deck** (effectively the old "Solo Mode", but using the Deck UI).
  - If N items are pinned: Show all N items in **The Deck** (tiled).

### 3.3. Dashboard Service Visibility

To support the development of `locald` itself, the `dashboard` service (when running in dev mode) must be visible in **The Rack** like any other project service. This allows developers to restart/debug the dashboard UI from within the dashboard itself (meta-hosting).

## 4. Implementation Details

### 4.1. Frontend (`locald-dashboard`)

- **Rack.svelte**:
  - Remove `solo` state.
  - Update row click handler to toggle `pinned`.
  - Update "System Normal" footer to toggle `pinned` with `locald`.
- **Deck.svelte**:
  - Handle the `locald` virtual service name.
  - Render a "Daemon Control Center" title for `locald`.

### 4.2. Backend (`locald-server`)

- **Logging**: Implement a `tracing` Layer that broadcasts daemon logs to the `ProcessManager`'s log channel.
- **Virtual Service**: Ensure the `locald` log stream is distinguishable from project services (e.g., `service: "locald"`).

### 4.3. Distribution & Dev Build Reality

The dashboard UI is served by the embedded static asset server in `locald-server`.

- **Default builds** embed the dashboard + docs assets (Cargo feature: `ui`).
- **Build-time enforcement**: missing/stale assets are rebuilt via `pnpm build` during compilation, and the build fails loudly if assets cannot be produced.
- **Headless builds** are explicit (`--no-default-features`) and do not embed UI assets.

## 5. User Experience

1.  **Monitoring the System**: User clicks "System Normal". The Deck opens with the Daemon Control Center.
2.  **Focusing a Service**: User clicks the `web` service row. The Deck updates to show `web` (and `locald` if it was already open).
3.  **Clearing Focus**: User clicks `web` again. It unpins. If nothing else is pinned, the view reverts to the global Stream.

This model is strictly additive and predictable. "What you pin is what you see."
