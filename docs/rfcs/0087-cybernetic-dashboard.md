---
title: "The Cybernetic Workspace: Rack, Stream, Deck"
stage: 3 # Recommended
feature: Dashboard
---

# RFC 0087: The Cybernetic Workspace (Rack, Stream, Deck)

## 1. Summary

This RFC proposes a complete overhaul of the `locald` dashboard UI paradigm, moving away from the traditional "Admin Panel" (Grid + Drawer) to a "Cybernetic Workspace" modeled after a mixing board and a tiling window manager. The new interface is composed of three primary elements: **The Rack** (Control), **The Stream** (Ambient Awareness), and **The Deck** (Focused Interaction).

## 2. The Diagnosis: "Does it have Conceptual Integrity?"

### 2.1. The Test

We examined the previous dashboard design (Grid View + Inspector Drawer) against the principle of **Conceptual Integrity**.

- **The Symptom**: The UI felt like a "laundry list" of features. "Here is a grid of cards. Click one to open a drawer. Click a tab to see logs. Click another tab to see config."
- **The Failure Mode**: This design forces the user to memorize a sequence of clicks to perform basic tasks. It treats the user as a **Viewer** of a static registry, rather than an **Operator** of a dynamic system.

### 2.2. The "Keyhole" Failure

Debugging a microservice architecture requires correlating events across multiple services (e.g., Frontend → Backend → Database). The "Drawer" pattern forces the user to view the system through a single "keyhole" at a time. This destroys context and makes cross-service debugging painful.

### 2.3. The "Garish" Failure

The previous design used color indiscriminately (colored chips, large status blocks). This created a "Wall of Noise" where everything screamed for attention, making it impossible to distinguish signal (errors, anomalies) from noise (static metadata).

## 3. The Prescription: "Make it Generative"

Instead of a list of features, we define three **Root Principles** from which the entire UI is derived.

### 3.1. Principle 1: The Clean Cockpit (Focus vs. Awareness)

**Generative Rule**: _Information should be ambient until it needs to be actionable. Controls should be invisible until they are needed._

- **Derivation (The Stream)**: By default, the center screen is a unified **Stream** of logs from all services. This provides "Ambient Awareness" (the hum of the machine).
- **Derivation (Solo Mode)**: Clicking a service in the sidebar activates **Solo Mode**. This is an _exclusive_ filter. The Stream instantly snaps to show _only_ that service. This provides "Focus".
- **Derivation (Hover Controls)**: Dangerous or cluttering actions (Pin, Restart, Disable) are hidden by default. They only appear when the user's mouse enters the specific "Action Area" of a service row. This keeps the interface calm.

### 3.2. Principle 2: High-Density Instrumentation (Professional Tool)

**Generative Rule**: _Screen real estate is precious. Metadata should be subtle; Content should be prominent. Use "Data Ink" efficiently._

- **Derivation (The Rack)**: The sidebar is not a navigation menu; it is a **Rack** of instruments.
  - **Eyebrow Tags**: Service types (`WEB`, `WORKER`) are displayed as tiny, monochrome "eyebrow" tags _above_ the service name. This saves horizontal space and removes the "garish" rainbow effect of colored chips.
  - **Sparklines**: Instead of a "CPU Graph" tab, a 16px high sparkline sits inline with the service name. It provides immediate, pre-attentive feedback on activity.
  - **Monochrome by Default**: Color is reserved strictly for **Status** (Green/Red dots) and **Content** (Log highlighting). The UI chrome itself is monochrome (`#52525b`) to recede into the background.
  - **Clean URLs**: Service links are displayed as "Clean URLs" (e.g., `https://api.shop.localhost`). Standard ports (`:80`, `:443`) are omitted to reduce visual noise.

### 3.3. Principle 3: Spatial Permanence (Muscle Memory)

**Generative Rule**: _Tools must not move. The workspace is a physical desk._

- **Derivation (The Deck)**: When a user needs to interact with multiple services simultaneously (The "Keyhole" solution), they **Pin** them. Pinned services open in **The Deck**—a tiling window manager that overlays The Stream.
- **Derivation (No Modals)**: There are no drawers or modals. The Rack is always on the left. The Work Area (Stream/Deck) is always in the center.

## 4. The Principles in Action (User Stories)

### 4.1. The "Morning Coffee" (Ambient Monitoring)

Alice starts `locald`. She sees **The Rack** on the left, showing all services green. **The Stream** in the center flows gently with "Health check passed" logs. She minimizes the window and starts coding.

### 4.2. The "Incident" (Soloing)

Bob sees a red dot on the `backend` service in **The Rack**. He clicks the `backend` row. **The Stream** instantly filters to show _only_ `backend` logs. He sees the stack trace.

### 4.3. The "Deep Dive" (Pinning)

Charlie needs to debug a race condition between `web` and `api`. He hovers over `web` and clicks the **Monitor** icon. He does the same for `api`.
The center view transforms into **The Deck**. Two large terminal windows appear side-by-side. He types into the `web` terminal to trigger a request and watches the `api` terminal for the reaction in real-time.

## 5. Implementation Strategy

### Phase 1: The Rack & Stream (Completed)

- [x] Implement "The Rack" with Eyebrow Tags and Sparklines.
- [x] Implement "The Stream" with Solo logic.
- [x] Implement "Clean Cockpit" hover states.

### Phase 2: The Deck (Completed)

- [x] Implement the "Pin" state in the UI store.
- [x] Build the Tiling Window Manager logic (using CSS Grid/Flexbox).
- [x] Embed `xterm.js` instances for pinned services (Placeholder implemented).

### Phase 3: The Cybernetics (Completed)

- [x] Connect Sparklines to real `locald-shim` metrics.
- [x] Add Keyboard Navigation (`j`, `k`, `Space`, `Enter`).
- [x] Full `xterm.js` integration for interactive terminals.

## 7. Artifacts

- **Living Prototype**: `locald-dashboard/src/routes/design-v2/+page.svelte`
  - Contains the reference implementation for the "Eyebrow Tag" layout, "Clean Cockpit" hover logic, and "Stream" filtering.
  - _Note_: This prototype uses mock data but demonstrates the intended CSS flexbox behavior and interaction model.
