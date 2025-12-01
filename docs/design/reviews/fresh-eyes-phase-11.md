# Fresh Eyes Review: Documentation & Persona Alignment

**Reviewer**: Fresh Eyes Mode
**Date**: 2025-11-30
**Subject**: Documentation Audit (Phase 11)

## Summary

I have audited the current documentation (`locald-docs/`) against our defined Personas. While the core structure is sound, there are significant gaps between the _implemented features_ (Phase 9 & 10) and the _documented workflows_.

## 1. The App Builder ("Regular Joe")

**Status**: ⚠️ **Friction Detected**

- **The "Init" Gap**: Phase 9 introduced `locald init` to interactively create projects, but the [Getting Started Guide](../locald-docs/src/content/docs/guides/getting-started.md) still instructs users to _manually_ create `locald.toml`. This defeats the purpose of the feature.
  - _Fix_: Update Getting Started to use `locald init`.
- **The "Monitor" Gap**: Phase 9 introduced `locald monitor` (TUI), a killer feature for this persona. It is completely absent from the introductory guides.
  - _Fix_: Add a "Monitoring Your App" section to Getting Started.
- **Missing Patterns**: The guide uses a Python example. We need a "Common Patterns" page showing `npm run dev`, `go run .`, etc., to reduce cognitive load.

## 2. The Power User ("System Tweaker")

**Status**: ⚠️ **Outdated Spec**

- **Missing `depends_on`**: Phase 10 added dependency resolution, but the [Configuration Reference](../locald-docs/src/content/docs/reference/configuration.md) does not list the `depends_on` field.
  - _Fix_: Add `depends_on` to the Service Options table.
- **Missing CLI Commands**: `locald init` and `locald monitor` need to be verified in the CLI reference.

## 3. The Contributor ("The Rustacean")

**Status**: 🟢 **Good Foundation**

- **Architecture**: The [Architecture Overview](../locald-docs/src/content/docs/internals/architecture.md) is accurate regarding the Client-Server model and IPC.
- **Update Needed**: It should briefly mention the new **Dependency Graph** logic in the Server component section to reflect Phase 10 changes.

## Recommendations

1.  **Rewrite Getting Started**: Center it around `locald init` and `locald monitor`.
2.  **Update Config Reference**: Add `depends_on`.
3.  **Create "Common Patterns"**: A new guide with copy-paste snippets for popular languages.
4.  **Update Architecture**: Mention topological sort/dependency resolution.
