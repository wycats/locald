# Phase 26 Walkthrough: Configuration & Registry

**RFC**: [docs/rfcs/0026-configuration-hierarchy.md](../../rfcs/0026-configuration-hierarchy.md)

## Changes

### 1. Global Configuration & Provenance

We started by refactoring the configuration system to support the new hierarchy and provenance tracking.

### Refactoring `locald-core`

- Split `locald-core/src/config.rs` into a module `locald-core/src/config/`.
- Created `locald-core/src/config/global.rs` to house the `GlobalConfig` struct.
- Created `locald-core/src/config/loader.rs` to implement `ConfigLoader` and `Provenance`.

### Provenance Tracking

We introduced a `Provenance` enum to track where configuration values originate:

```rust
pub enum Provenance {
    Default,
    Global(PathBuf),
    Context(PathBuf),
    Workspace(PathBuf),
    Project(PathBuf),
    EnvVar(String),
}
```

The `ConfigLoader` now has an `explain_global(key)` method that returns the provenance of a specific key.

### CLI: `locald config show`

We added a new `config` command to the CLI:

- `locald config show`: Dumps the resolved configuration (currently just Global).
- `locald config show --provenance`: Displays the value and its source for each key.

Example output:

```text
Global Configuration:
  server.privileged_ports = true (default)
  server.fallback_ports = true (env:LOCALD_FALLBACK_PORTS)
```

### 2. Project Registry

We implemented the Registry system to track known projects and support "Always Up" functionality.

### Core Logic (`locald-core`)

- Created `locald-core/src/registry.rs` containing the `Registry` and `ProjectEntry` structs.
- The registry is persisted to `~/.local/share/locald/registry.json`.
- Added `Registry::load()`, `save()`, `register()`, `pin()`, `unpin()`, and `clean()`.

### IPC Protocol

Updated `locald-core/src/ipc.rs` with new request/response variants:

- `RegistryList` -> `RegistryList(Vec<ProjectEntry>)`
- `RegistryPin { path }` -> `Ok`
- `RegistryUnpin { path }` -> `Ok`
- `RegistryClean` -> `RegistryCleaned(usize)`

### Server Integration (`locald-server`)

- Updated `ProcessManager` to hold an `Arc<Mutex<Registry>>`.
- On `start(path)`, the project is automatically registered (added/updated in the registry).
- Implemented handlers for the new IPC requests.

### CLI Commands (`locald-cli`)

Added the `registry` subcommand:

- `locald registry list`: Lists all known projects with their status (pinned/last seen).
- `locald registry pin [path]`: Marks a project as "pinned" (intended for autostart).
- `locald registry unpin [path]`: Unpins a project.
- `locald registry clean`: Removes projects that no longer exist on disk.

