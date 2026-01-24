# RFC 0085: Workspace Support (Multi-Project Configuration)

## Context

Currently, `locald` operates on a single `locald.toml` file, which defines a single "Project". To run multiple projects simultaneously (e.g., a monorepo with a backend and a frontend, or the `dotlocal` repo itself which contains the main project and the `locald-dashboard` project), the user must:

1.  Run `locald up` in the first project.
2.  Open a new terminal.
3.  Run `locald up` in the second project.
4.  (Optional) Use `locald registry pin` to keep them running.

This friction violates **Axiom 1: Zero-Friction Start**. A developer working on a complex system should be able to bring up the entire environment with a single command.

## Problem

The user wants to run the development version of the dashboard (`locald-dashboard`) alongside the main `dotlocal` services without manually managing separate `locald` invocations.

## Proposal

Introduce a `[workspace]` section to `locald.toml`. This allows a root configuration to include other `locald.toml` files (projects) as members of a workspace.

### Syntax

```toml
# locald.toml (root)

[project]
name = "my-monorepo"

# Define workspace members
[workspace]
members = [
    "packages/frontend",
    "packages/backend",
    "locald-dashboard"
]
```

### Behavior

When `locald up` is run in a directory containing a `[workspace]` section:

1.  **Root Project**: The root project is registered and started as usual.
2.  **Member Discovery**: `locald` iterates through the `members` list.
3.  **Recursive Loading**: For each member, it looks for a `locald.toml` in the specified subdirectory.
4.  **Registration**: Each member is registered as a separate project in the `locald` registry, just as if `locald up` had been run in that directory.
5.  **Lifecycle**:
    *   `locald up` starts the root and all members.
    *   `locald stop` (without arguments) stops the root and all members? Or maybe we need `locald workspace stop`?
    *   Ideally, `locald up` ensures the *entire workspace* is running.

### Benefits

1.  **Single Command**: `locald up` brings up the full stack.
2.  **Monorepo Support**: First-class support for modern monorepo structures.
3.  **Development of locald**: Specifically helps us develop `locald` itself by letting us run the dashboard dev server as part of the main loop.

## Alternatives

1.  **`[[project]]` array**: Allow defining multiple projects inline in one file.
    *   *Cons*: Can make the file huge. Doesn't play well with existing `locald.toml` files in subdirectories.
2.  **Shell Scripts**: `up.sh` that calls `locald up` multiple times.
    *   *Cons*: "Anti-Pattern: Manual Process Management via Shell".

## Implementation Plan

1.  **Config Parsing**: Update `locald-core` config structs to include `[workspace]`.
2.  **Loader Logic**: Update `locald-server`'s config loader to handle `members`.
3.  **Registry Interaction**: The loader needs to iterate and register multiple projects.
