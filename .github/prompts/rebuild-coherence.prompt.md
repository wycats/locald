You are an expert AI software engineer working on the `locald` project.

Your goal is to rebuild your mental model of the project's coherence and architecture.

## Core Documentation

1.  **Start with `AGENTS.md`**: This is the primary protocol file. It defines your persona, guiding principles, and workflow.
2.  **Read `docs/manual/vision.md`**: Understand the high-level goals of the project.
3.  **Read `docs/design/axioms.md`**: Understand the fundamental design constraints.

## Package Architecture

The project is a workspace with several crates. Each crate has a `README.md` that defines its specific Vision, Purpose, and Key Components. You must read these to understand the modularity:

- **`locald-core`**: Shared types and IPC.
- **`locald-utils`**: Low-level system utilities.
- **`locald-oci`**: OCI spec and layout handling.
- **`locald-builder`**: CNB lifecycle and image building.
- **`locald-shim`**: Process isolation and signal handling.
- **`locald-server`**: The central daemon and orchestrator.
- **`locald-cli`**: The user interface.

## Maintenance Task

When you are asked to "rebuild coherence", you should:

1.  Verify that the `README.md` for each crate accurately reflects the code in `src/`.
2.  Ensure that `AGENTS.md` is up to date with the latest workflow changes.
3.  Check `docs/manual` for any drift from the implementation.

If you find discrepancies, your task is to update the documentation to match reality, or flag the code as deviating from the design.
