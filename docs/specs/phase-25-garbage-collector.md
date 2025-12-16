# Phase 25: Garbage Collector Implementation

**Goal**: Implement the Mark-Sweep Garbage Collector defined in [RFC 0095](../rfcs/0095-garbage-collector.md).

## 1. Core Logic (`locald-core`)

- [ ] Update `Registry` struct to include `last_seen` (already exists, verify usage).
- [ ] Implement `Registry::prune(active_paths: HashSet<PathBuf>, ttl: Duration) -> Vec<PathBuf>`.
  - Logic: Remove if `!pinned && !active && !present && !recent`.
  - Return list of removed paths for cleanup.

## 2. Server Logic (`locald-server`)

- [ ] Implement `Manager::run_gc(force: bool)`.
  - [ ] **Mark**: Call `registry.prune`.
  - [ ] **Sweep (Registry)**: Delete state dirs for pruned projects.
  - [ ] **Sweep (Orphans)**: Iterate `projects/` dir and delete any folder not in Registry.
- [ ] Add `Manager::audit() -> AuditReport`.
  - Returns list of { Active, Dormant, Hollow, Missing } projects.
- [ ] Add background task (Heartbeat) to run `run_gc(false)` every 24h.
- [ ] Add "Lazy Sweep" on server startup.

## 3. CLI (`locald-cli`)

- [ ] Update `locald registry clean` to call `run_gc(force=true)`.
- [ ] Add `locald registry audit` command.
  - Displays table of project states.
  - Interactive prompt to clean up "Hollow" or "Dormant" projects.

## 4. Testing

- [ ] Unit tests for `Registry::prune` logic (TTL, pinning).
- [ ] Integration test:
  1. Start project.
  2. Stop project.
  3. Delete source dir.
  4. Verify state dir remains (TTL).
  5. Fast-forward time (mock) or force GC.
  6. Verify state dir deleted.
