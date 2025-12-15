# Architecture: State Management

This document describes how `locald` manages persistent state, including project data, logs, and the global registry.

## 1. The Global State Directory

`locald` adheres to the **XDG Base Directory Specification** for storing data. It does **not** pollute the user's project directories with build artifacts or logs.

- **Location**: `XDG_DATA_HOME/locald` (typically `~/.local/share/locald` on Linux).
- **Structure**:
  ```
  ~/.local/share/locald/
  ├── registry.json       # Global list of known projects
  ├── server-state.json   # Persisted runtime state (PIDs, ports)
  └── projects/           # Per-project data
      └── <name>-<hash>/  # Unique directory for each project
          └── .locald/    # The "virtual" .locald directory
              ├── logs/   # Service logs
              └── run/    # PID files, sockets
  ```

### Project Isolation

Each project is assigned a unique directory based on the **SHA-256 hash of its absolute path**. This ensures that:

1.  Two projects with the same name (e.g., `~/work/blog` and `~/personal/blog`) do not collide.
2.  Moving a project changes its hash, effectively treating it as a new project (preventing stale state from corrupting the new location).

## 2. The Registry

The **Registry** (`registry.json`) is the source of truth for all projects `locald` has ever seen.

### Schema

```json
{
  "projects": {
    "/home/user/code/my-app": {
      "path": "/home/user/code/my-app",
      "name": "my-app",
      "pinned": false,
      "last_seen": "2025-12-11T10:00:00Z"
    }
  }
}
```

### Lifecycle

- **Registration**: Occurs automatically when `locald up` or `locald start` is run in a directory.
- **Pruning**: Projects that are removed from disk are eventually cleaned up by the Garbage Collector (see below).
- **Pinning**: Users can "pin" a project (`locald registry pin .`) to prevent it from being garbage collected, even if the directory is missing (e.g., on a detached drive).

## 3. Garbage Collection (Mark-Sweep)

To prevent the global state directory from growing indefinitely, `locald` employs a **Mark-Sweep Garbage Collector**.

- **Mark (Root Set)**: Projects are preserved if they are:
  - **Pinned**
  - **Active** (Currently running services)
  - **Present** (Source directory exists)
  - **Recent** (Last seen within the TTL window, default 7 days)
- **Sweep**: Any directory in `~/.local/share/locald/projects/` that does not correspond to a Marked project is deleted.

This ensures that `locald` is self-cleaning while being tolerant of temporary filesystem changes.
