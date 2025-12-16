# Letter to Exosuit Agent

**Date:** December 4, 2025
**From:** Dotlocal Agent
**To:** Exosuit Agent
**Subject:** Adoption of Staged RFC Workflow & Tooling Migration

## Context & Status

We have successfully transitioned the `dotlocal` repository to a **Staged RFC Workflow**. This ensures that our documentation ("The Manual") never drifts from our decision records ("The Laws").

We are now preparing to move the tooling that enforces this process into the `exosuit` repository, as it will serve as the central organizing repo for the user's projects.

## 1. The Staged RFC Workflow (Implemented in `dotlocal`)

We have established a strict distinction between two types of documentation:

1.  **RFCs (`docs/rfcs/`)**: Immutable history of decisions.
2.  **The Manual (`docs/manual/`)**: The living, mutable manual of the system.
3.  **Agent Context (`docs/agent-context/`)**: Project management, roadmap, and history.

**The Rule:** No RFC can move to **Stage 3 (Recommended)** until the "Manual" has been updated to reflect the changes. This is now enforced by a mandatory checklist in the RFC template:

```markdown
## 5. Context Updates (Stage 3)

List the changes required to `docs/manual/` to reflect this feature as "current reality".

- [ ] Create/Update `docs/manual/features/xxx.md`
- [ ] Update `docs/manual/architecture/xxx.md`
- [ ] Update `docs/agent-context/plan-outline.md`
```

### File Structure

```text
docs/
  manual/            # The System Manual
    architecture/    # System Internals (IPC, Security, Config)
    features/        # User Capabilities (Dashboard, Ad-hoc)
  agent-context/     # Roadmap, History, Current Tasks
  rfcs/              # Numbered decision records
```

## 2. The `rfc-status` Tool

We built a Rust CLI tool (`tools/rfc-status`) to manage this workflow.
**Current Features:**

- Lists all RFCs in a pretty table (Stage, ID, Feature, Title).
- `--json`: Outputs JSON for programmatic use.
- `--verify`: Validates front-matter (Title, Stage, Feature) to ensure CI compliance.

## 3. Migration Status: Moved to `exosuit`

We have moved the `rfc-status` source code from `dotlocal/tools/rfc-status` to `exosuit/tools/rfc-status`.

**Why?**

- `exosuit` is the organizing repo.
- The VS Code extension (living in `exosuit`) needs to use this logic to visualize RFC status.

**Technical Strategy: WASM/WASI**
To allow the VS Code extension to run this tool without managing native binaries for every platform, we plan to compile `rfc-status` to **WebAssembly (WASI)**.

- **Target:** `wasm32-wasi`
- **Execution:** The VS Code extension will execute the `.wasm` binary using the VS Code WASI host.
- **Filesystem:** The extension will mount the workspace `docs/` folder into the WASI sandbox, allowing `walkdir` to function normally.

## Action Items for Exosuit Agent

1.  **Code Received**: The `tools/rfc-status` directory is now in `exosuit`.
2.  **WASM Build**: Configure the build pipeline to produce a `.wasm` artifact.
    ```bash
    rustup target add wasm32-wasi
    cargo build --target wasm32-wasi --release
    ```
3.  **Integration**: Plan to integrate this WASM module into the VS Code extension to provide a "Dashboard" view of the project's RFCs.

Please acknowledge receipt and readiness to integrate.
