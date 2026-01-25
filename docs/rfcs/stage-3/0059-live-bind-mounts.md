---
title: "Live Bind Mounts for Development"
stage: 1 # 0: Strawman, 1: Accepted, 2: Available, 3: Recommended, 4: Stable
feature: Hot Reloading
---

# RFC 0059: Live Bind Mounts for Development

## 1. Summary

This RFC establishes the "Split Lifecycle" strategy for `locald`:

1.  **Builds** use a **Snapshot** (copy) of the source code to ensure isolation and reproducibility.
2.  **Runs** use a **Bind Mount** of the source code to ensure interactivity (hot reloading).

It also proposes the "Overlay Mount" pattern to resolve conflicts between host source code and container-native build artifacts (like `node_modules`).

## 2. Motivation

### The Conflict

We have two competing needs:

1.  **Isolation (Build)**: When building, we don't want the process to modify the user's source code or be affected by ignored files. We also want high I/O performance, which is best achieved by copying files into the VM/Container filesystem.
2.  **Interactivity (Run)**: When running, the user expects changes in their editor to immediately reflect in the running service.

### The Solution: Split Lifecycle

We explicitly distinguish between the filesystem strategy for Building and Running.

- **Build**: `rsync` source -> Temporary Directory -> Bind Mount to Container.
- **Run**: Bind Mount source -> Container.

This is now codified in **Axiom 13: The Development Loop**.

### The Remaining Problem: Artifact Incompatibility

Even with the Split Lifecycle, the **Run** phase has a problem.
If we bind-mount the root (`.`) to `/workspace`, the container sees the host's `node_modules`. This often leads to:

- `ELF header mismatch` errors (wrong architecture/OS).
- Missing dependencies (if the host hasn't run `npm install`).
- Pollution of the host directory (if the container writes to `node_modules`).

## 3. Detailed Design

### Proposed Strategy: "Overlay" Mounts

We need a way to mount the source code _live_, but "mask" or "replace" the incompatible directories with the valid ones generated during the build.

#### 1. The "Volume Overlay" Pattern

1.  **Build Phase**: The CNB build produces a valid `node_modules` (or `target`, `venv`, etc.) inside the image/layer.
2.  **Preparation**: Before running, `locald` extracts these valid artifacts to a managed cache directory (e.g., `.locald/overlays/<service>/node_modules`).
3.  **Run Phase**:
    - Bind mount Host `.` -> Container `/workspace` (Read-Write).
    - Bind mount Cache `.locald/overlays/<service>/node_modules` -> Container `/workspace/node_modules` (Read-Write).

**Result**:

- The application sees the live source code from the host.
- The application sees the valid, container-compatible dependencies from the build.
- Writes to `node_modules` inside the container go to the cache, not the host.

### Technical Challenges to Investigate

1.  **Identification**: How do we know _which_ directories need to be overlaid?
    - _Idea_: CNB Buildpacks might provide metadata?
    - _Idea_: Heuristics based on language (Node -> `node_modules`, Rust -> `target`, Python -> `__pycache__`, `.venv`).
    - _Idea_: User configuration in `locald.toml`.
2.  **Synchronization**: If the user adds a dependency in `package.json` on the host, the container's `node_modules` (in the overlay) is now stale.
    - The user would likely need to run a command to update it, or `locald` detects the change and triggers a rebuild/re-sync.
3.  **File Permissions**:
    - The container runs as a specific UID (mapped via User Namespaces).
    - The host files are owned by the user.
    - The overlay files are created by `locald`.
    - We must ensure the container process has R/W access to both the bind-mounted source and the overlay mounts.

### Alternative: File Syncing

Instead of bind mounts, we could use a file watcher (like `notify`) to detect changes on the host and `rsync` them into the running container (or the directory it is bound to).

- _Pros_: Simpler isolation. No mount masking needed.
- _Cons_: Latency. "Eventual consistency". High CPU usage for watchers.

## 4. Implementation Plan (Stage 2)

- [ ] **Prototype**: Create a manual `runc` bundle that implements the "Volume Overlay" pattern for a simple Node.js app.
- [ ] **Config Design**: Define how users or buildpacks specify which directories to overlay.
- [ ] **Integration**: Update `locald-builder` to support generating `config.json` with these complex mount configurations.

## 5. Context Updates (Stage 3)

- [ ] Update `docs/manual/features/hot-reloading.md`.
- [ ] Update `docs/manual/architecture/container-runtime.md`.

## 6. Drawbacks

- **Complexity**: Managing multiple mounts and keeping them in sync is significantly more complex than "copy and run".
- **Performance**: Docker/runc bind mounts can be slow on non-Linux filesystems (macOS/Windows), though `locald` targets Linux first.

## 7. Unresolved Questions

- Does `runc` support mounting a directory _over_ a subdirectory of another bind mount? (Yes, standard Linux mount semantics allow this).
- How do we handle "deletion"? If a user deletes a file on the host, it disappears from the container (Good).

## 8. Future Possibilities

- **Dev Containers**: This moves us closer to a full "Dev Container" experience where the entire dev environment lives in the container.
