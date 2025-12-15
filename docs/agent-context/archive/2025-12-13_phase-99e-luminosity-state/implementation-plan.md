# Implementation Plan - Phase 99e: Luminosity & State

**Goal**: Refine the dashboard UI to improve contrast, accessibility, and state visibility. This phase focuses on "brightening the lights" and ensuring active states are clearly communicated.

## 1. Interaction: The "Active" Toolbar

**Objective**: Create a direct visual link between the Sidebar Row and the Open Panel/Log View.

- **Logic**: If a service is **Pinned/Active** (displayed in the log view), its toolbar controls must be persistent.
- **Changes**:
  - Force Toolbar opacity to 100% when `active == true`.
  - Style the Monitor Icon as "Active" (`text-blue-400`, `bg-white/10`) when active.

## 2. Global Contrast Tuning

**Objective**: Improve accessibility and visual hierarchy by brightening inactive elements.

- **Sidebar Inactive Items**: Change from `text-zinc-700` (approx) to `text-zinc-400`.
- **Sidebar Headers**: Change to `text-zinc-500`.
- **Badge Text**: Use the **400** shade for better contrast against black backgrounds (`text-blue-400`, `text-purple-400`, `text-amber-400`).
- **Log Timestamps**: Ensure they are `text-zinc-500`.

## 3. Sidebar Hover State

**Objective**: Provide a clear "cursor" feel.

- **Row Hover**: Set background to `bg-white/5` (or `bg-zinc-800/50`) on hover.

## 4. Verification

- Visual inspection of the dashboard.
- Verify accessibility improvements.
