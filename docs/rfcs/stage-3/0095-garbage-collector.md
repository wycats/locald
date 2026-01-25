# RFC 0095: The Garbage Collector Protocol

- **Date:** 2025-12-11
- **Status:** Accepted
- **Area:** Core / State Management

## Summary

We are implementing a **Mark-Sweep Garbage Collector** for `locald`'s global state directory. Instead of a naive "delete if missing" approach, we introduce a biological lifecycle for project data: projects are "Live" (Root Set) based on presence, activity, or recency (TTL). Any data falling outside this Root Set is treated as "Entropy" and swept away by a periodic heartbeat.

## Motivation: The Entropy Problem

In [RFC 0094](./0094-global-state-directory.md), we moved project state from the user's visible workspace (`.locald`) to a hidden global store (`XDG_DATA_HOME/locald/projects/...`).

This solved the "pollution" problem but introduced an "accumulation" problem. When state was local, `rm -rf project` cleaned up the state naturally. Now, deleting a project folder leaves its ghost behind in the global store. Over time, this hidden directory accumulates "digital sediment"—logs, build artifacts, and databases from long-forgotten experiments.

We cannot simply delete data the moment a project directory is unmounted or moved (the "Panic Delete" strategy). That would be catastrophic for users working on removable drives or temporarily reorganizing folders.

We need a system that tolerates ambiguity but eventually enforces order. We need a Garbage Collector.

## Design: The Mark-Sweep Protocol

We adopt the classic **Mark-Sweep** algorithm from memory management, applied to the file system.

### 1. The Mark Phase: Defining the Root Set

The **Root Set** defines what data is "Live". If a project is in the Root Set, its state directory is **Marked** (preserved).

A project is in the Root Set if it meets **ANY** of the following criteria:

1.  **Pinned**: The user has explicitly pinned the project (`locald registry pin .`). This is the "Golden Spike" that prevents deletion regardless of other factors.
2.  **Active**: The project has services currently running. We query the `ProcessManager` to ensure we never pull the rug out from under a running process.
3.  **Present**: The project's source directory exists on disk at the registered path.
4.  **Recent (TTL)**: The project was "last seen" within the **TTL Window** (default: 7 days). This is the "Grace Period". Even if the source directory is missing (e.g., unmounted drive), we hold the state for a week before declaring it dead.

### 2. The Sweep Phase: Reclaiming Entropy

The Sweep Phase is the physical reclamation of disk space. It operates on the principle of **Allowlisting**:

1.  **Calculate Allowlist**: We iterate the Registry. For every project in the Root Set, we calculate its expected global state path (`~/.local/share/locald/projects/<name>-<hash>`).
2.  **Walk the Heap**: We iterate the physical `projects/` directory in `XDG_DATA_HOME`.
3.  **Identify Orphans**: Any directory found on disk that is **NOT** in the Allowlist is an Orphan.
4.  **Reclaim**: We `rm -rf` the Orphan.

This approach is robust against Registry corruption. If the Registry is lost, we don't delete everything; we default to safety (or we can rebuild the Registry from the state directories if we add back-links). But for now, the Registry is the Source of Truth.

### 3. Automation: The Heartbeat

Garbage collection is a system maintenance task, not a user chore.

- **Lazy Sweep (Startup)**: On `locald server` startup, we run a low-priority sweep to clean up any mess left by a previous crash.
- **The Heartbeat**: A background task runs every 24 hours to perform a full Mark-Sweep cycle.
- **Manual Trigger**: `locald registry clean` (or `locald admin gc`) triggers an immediate sweep.

### 4. The Audit Protocol (Manual Intervention)

While the Auto-GC is conservative (safety first), users need a way to identify "Dormant" or "Hollow" projects that are technically "Live" but practically dead.

We introduce `locald registry audit` which categorizes projects:

1.  **Active**: Currently running.
2.  **Dormant**: Present, but `last_seen` > 30 days.
3.  **Hollow**: Directory exists, but no `locald.toml` or `Procfile` found.
4.  **Missing**: Directory gone (waiting for TTL).

The command will display disk usage for each and offer an interactive cleanup wizard.

## Implementation Details

### The `Registry::prune` Method

We evolve the existing `prune_missing_projects` into a more sophisticated `prune` method:

```rust
pub fn prune(&mut self, active_projects: &HashSet<PathBuf>, ttl: Duration) -> Vec<PathBuf> {
    let now = SystemTime::now();
    let mut to_remove = Vec::new();

    for (path, entry) in &self.projects {
        let is_pinned = entry.pinned;
        let is_active = active_projects.contains(path);
        let is_present = path.exists();
        let is_recent = entry.last_seen.elapsed().unwrap_or_default() < ttl;

        if !is_pinned && !is_active && !is_present && !is_recent {
            to_remove.push(path.clone());
        }
    }

    // ... remove from map and return list of removed paths ...
}
```

### The `Manager::gc` Method

The `ProcessManager` orchestrates the GC:

1.  Acquire Registry lock.
2.  Get list of currently running project paths.
3.  Call `registry.prune(active_paths, ttl)`.
4.  For each removed project, calculate `get_state_dir(path)` and delete it.
5.  **Crucially**: Also scan the `projects/` directory for _any_ folder that doesn't match the remaining registry entries (handling the "Registry Drift" case).

## Drawbacks & Risks

- **The "Zombie" Risk**: If we delete a state directory while a process is secretly using it (e.g., a detached process we lost track of), that process might crash or corrupt data.
  - _Mitigation_: The "Active" check relies on `ProcessManager`'s knowledge. If `locald` restarts, it recovers state. If a process is truly orphaned from `locald`, it is effectively rogue. We could try to detect open file handles, but that is complex. The "Active" check is sufficient for 99% of cases.
- **TTL Confusion**: Users might be confused why disk space isn't freed immediately after deleting a project.
  - _Mitigation_: `locald registry clean` can support a `--force` flag to ignore TTL.

## Alternatives Considered

- **Reference Counting**: Add a `.ref` file in the project directory pointing to the global state.
  - _Rejection_: Requires writing to the user's source directory, which violates the "Zero Pollution" axiom of RFC 0094.
- **Manual Only**: Only clean when the user asks.
  - _Rejection_: Leads to unbounded disk usage. Users forget. Systems should be self-maintaining.
