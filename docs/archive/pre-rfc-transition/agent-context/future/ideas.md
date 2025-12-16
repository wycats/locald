<!-- agent-template start -->

# Ideas

Ideas for future work.

<!-- agent-template end -->

(All previous ideas have been triaged into `docs/agent-context/plan-outline.md` as of Phase 19/20+)

## Configuration

- **Root Config / Subdirectory Execution**: Allow a `locald.toml` in the root of a repo to define commands that run in a specific subdirectory. This is useful for monorepos or projects where the build tooling lives in a subfolder but the "project" is the repo. Related to Constellations.

## CLI Ergonomics

- **Context-Aware Bare Commands**: Commands like `locald restart` or `locald status` (without arguments) should default to operating on the service defined in the current directory (if any), rather than requiring a service name or showing global status.
