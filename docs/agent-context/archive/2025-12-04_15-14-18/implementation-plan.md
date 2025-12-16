# Implementation Plan - Phase 26: Configuration & Constellations

## 1. Design Proposal (RFC)
*Goal: Answer open questions and define the exact behavior before coding.*

- Create `docs/design/epoch-3-hybrid/rfc-configuration-hierarchy.md`.
- **Key Decisions needed:**
    - **Discovery**: How to find "Context" config without expensive tree walking? (Proposal: Only look in parent dirs up to `$HOME` or git root).
    - **Naming**: "Constellation" vs "Workspace". (Proposal: "Workspace" aligns better with Rust/JS ecosystems).
    - **Schema**: Define the schema for `GlobalConfig` and `WorkspaceConfig`.
    - **Registry**: Define the JSON structure for `registry.json`.

## 2. Global Configuration
*Goal: Allow user-wide defaults.*

- Define `GlobalConfig` struct in `locald-core`.
- Implement `load_global_config()` in `locald-server`.
- Support settings:
    - `default_domain` (override `.localhost`)
    - `log_level`
    - `theme` (for dashboard)

## 3. The Registry
*Goal: Remember projects and support "Always Up".*

- Define `Registry` struct:
    ```rust
    struct Registry {
        projects: HashMap<PathBuf, ProjectMetadata>
    }
    struct ProjectMetadata {
        last_seen: DateTime<Utc>,
        always_up: bool,
        name: String,
    }
    ```
- Implement `RegistryManager` to save/load from `~/.local/share/locald/registry.json`.
- Update `locald-server` to register projects on startup/discovery.
- Implement "Always Up" logic: On daemon start, iterate registry and start `always_up` projects.

## 4. Cascading Configuration
*Goal: Merge configurations from multiple sources.*

- Implement a `ConfigLoader` that:
    1. Loads Global Config.
    2. Walks up from the project path to find "Context" configs (e.g., `.locald.toml` in parent dirs).
    3. Loads Project Config.
    4. Merges them (Project wins).
- **Refactor**: Update `ServiceConfig` to be the result of this merge.

## 5. CLI & Dashboard
*Goal: Expose the new capabilities.*

- **CLI**:
    - `locald config show`: Dump the merged config.
    - `locald project pin`: Set `always_up = true`.
    - `locald project unpin`: Set `always_up = false`.
- **Dashboard**:
    - Visual indication of "Pinned" services.
    - Grouping by Workspace/Folder.
