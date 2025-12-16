# Task List: Structured Cgroup Hierarchy (Phase 99)

## Current Focus

- [ ] **Phase 99: Structured Cgroup Hierarchy**.
  - [x] **RFC 0099**: Design & Validation (Stage 3).
  - [x] **Manual**: Create `docs/manual/architecture/resource-management.md`.
  - [x] **Axiom**: Add [Axiom 14: Precise Abstractions](../design/axioms/architecture/14-precise-abstractions.md).
  - [ ] **Implementation**:
    - [ ] Update `locald-server` to generate cgroup paths.
    - [ ] Update `locald-shim` to implement "The Anchor" (Systemd Unit).
    - [ ] Update `locald-shim` to implement "The Driver" (Direct Cgroup).
    - [x] Container execution via `libcontainer` (completed in Phase 97).

## Phase 94: Builder Permissions & Context (Completed)

- [x] **Feature**: Proxy Lazy Loading (Phase 93).
  - [x] Implement "Loading..." page for slow services.
  - [x] Verify with `examples/shop-frontend`.
- [x] **Fix**: `cargo install-locald` alias.
  - [x] Add `--locked --offline` to prevent unnecessary rebuilds.
- [x] **Bug**: Builder Permission Denied (Regression).
  - [x] **Diagnosis**: `locald-builder` fails to clean up `rootfs` due to `runc` artifacts (e.g., `/run/shm`) owned by root.
  - [x] **Fix**: Added `ShimRuntime::cleanup_path` fallback in `image.rs`, `lifecycle.rs`, `bundle_source.rs` and `locald-server/src/runtime/process.rs`.
  - [x] **Verification**: Verified that `locald` successfully cleans up a root-owned file in `.locald/build`.
- [x] **Bug**: Dev Loop Instability (Fork Bomb).
  - [x] **Diagnosis**: Recursive execution loop between `locald` and `locald-shim` when `locald` fails to detect privileges.
  - [x] **Fix**: Implemented **RFC 0096: Leaf Node Axiom**.
    - [x] `locald-shim` is now a leaf node (never execs `locald`).
    - [x] Implemented `bind` command in shim for FD passing.
    - [x] Updated `locald-server` to use shim for privileged ports via `SCM_RIGHTS`.
    - [x] Added architecture compliance tests.
- [x] **Context**: Update Agent Context.
  - [x] Update `changelog.md` with Phase 94.
  - [x] Update `task-list.md` (this file).

## Phase 93: Proxy Lazy Loading (Completed)

**Goal**: Improve perceived performance for slow-booting web services.

- [x] **RFC 0093**: Create and implement.
- [x] **Implementation**: Intercept HTML requests and serve loading page.

## Phase 87: Cybernetic Dashboard (RFC 0087)

**Goal**: Implement the "Rack, Stream, Deck" paradigm to improve information density and workflow.

- [x] **RFC 0087**: Advance to Stage 2 (Implementation).
- [x] **Frontend Components**:
  - [x] `Rack.svelte`: High-density sidebar with sparklines.
  - [x] `Stream.svelte`: Unified log stream with "Solo" mode.
  - [x] `Deck.svelte`: Tiling window manager for pinned services.
- [ ] **Data Integration**:
  - [ ] Ensure `services` store provides necessary data (CPU usage for sparklines?).
  - [x] Ensure `logs` store supports unified streaming.

## Phase 81: Dashboard Refinement (Completed)

**Goal**: Elevate the dashboard from a passive viewer to a robust, interactive workspace.

- [x] **Frontend Architecture**: Implement the new SvelteKit-based dashboard structure.
  - [x] `Sidebar.svelte`: Navigation and global controls.
  - [x] `ServiceGrid.svelte`: Project-centric view of services.
  - [x] `ServiceCard.svelte`: Individual service status and quick actions.
  - [x] `InspectorDrawer.svelte`: Detailed view for a selected service (logs, config, env).
- [x] **Backend Integration**: Update `locald-server` to serve the new dashboard.
  - [x] Update `build.rs` to track and embed dashboard assets.
  - [x] Create `scripts/build-assets.sh` to handle the build process.
  - [x] Verify `cargo install-locald` includes the assets.
