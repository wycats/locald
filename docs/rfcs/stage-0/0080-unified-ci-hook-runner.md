# RFC 0080: Unified CI/Hook Runner (Strawman)

## Status

- **Stage**: 0 (Strawman)
- **Date**: 2025-12-09

## Context

We currently maintain code quality checks in two places: `lefthook.yml` and `.github/workflows/ci.yml`. This leads to configuration drift.

Additionally, some checks have **cross-language** dependencies (Rust build scripts that invoke `pnpm`) and **privileged steps** (installing the setuid shim for E2E tests). These constraints make it easy for CI to drift from local expectations unless they are explicitly modeled.

## Proposal

Create a standalone tool, `hook-bridge`, that serves as the **single source of truth**.
`lefthook.yml` (Source) -> `hook-bridge` -> `GitHub Actions Matrix` (Output)

## Technical Architecture

### 1. Core (Rust)

- **Parser**: Reads `lefthook.yml`. It leverages the fact that Lefthook ignores unknown keys to support custom metadata.
- **Planner**: Generates a matrix of commands.
- **Granularity**: **Per-command**. If `pre-commit` has `fmt` and `clippy`, the planner outputs two distinct jobs.

### 2. Distribution (WASM Component)

- **Build Target**: `wasm32-wasi`.
- **NPM Integration**: Use **`jco`** (Bytecode Alliance) to transpile the WASM component into a native Node.js module.
  - _Why?_ Removes the need for experimental Node flags or manual WASI polyfills.
  - _Result:_ A zero-dependency npm package that runs the Rust logic at near-native speed.

### 3. Context Adaptation (The "Magic")

The tool adapts command templates using a custom `ci` block in `lefthook.yml`.

**Example Configuration:**

```yaml
pre-commit:
  commands:
    fmt:
      glob: "*.rs"
      # Local: Runs on staged files
      run: cargo fmt --all {staged_files}
      # CI Metadata (Ignored by standard lefthook)
      ci:
        # CI: Replaces {staged_files} with "."
        staged_files: "."
        # CI: Appends these arguments
        append_args: "-- --check"
```

**Logic:**

1.  **Local**: Standard `lefthook` binary runs `cargo fmt --all file1.rs file2.rs`.
2.  **CI**: `hook-bridge` reads the config.
    - Sees `ci.staged_files` override.
    - Constructs command: `cargo fmt --all . -- --check`.

## Integration Workflow

**In GitHub Actions:**

```yaml
jobs:
  plan:
    runs-on: ubuntu-latest
    outputs:
      matrix: ${{ steps.plan.outputs.matrix }}
    steps:
      - uses: actions/checkout@v4
      - run: npx hook-bridge plan --format=github-matrix > matrix.json
      - id: plan
        run: echo "matrix=$(cat matrix.json)" >> $GITHUB_OUTPUT

  check:
    needs: plan
    strategy:
      matrix: ${{ fromJson(needs.plan.outputs.matrix) }}
    steps:
      - uses: actions/checkout@v4
      # The tool knows how to run the command for the current matrix item
      - run: npx hook-bridge run ${{ matrix.id }} --mode=ci
```

## Constraints to Model (CI Reality)

Any generated workflow must be able to represent:

- **Node/pnpm availability**: some Rust builds require `pnpm` during `build.rs` when UI assets are embedded.
- **Coverage instrumentation**: `cargo-llvm-cov` runs are slower and can appear “stuck” during linking; CI should enable progress output.
- **Privileged shim install**: E2E tests require `locald admin setup` to install the setuid shim before running.
- **Coverage profile integrity**: instrumented processes produce `*.profraw` on clean exit; test harnesses should avoid abrupt termination of background daemons to prevent corrupt profiles.

## Next Steps

1.  Prototype the `lefthook.yml` parser in Rust.
2.  Setup the `wasm32-wasi` build pipeline.
3.  Create the npm wrapper proof-of-concept.
