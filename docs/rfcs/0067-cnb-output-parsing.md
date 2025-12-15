# RFC 0067: CNB Output Parsing Strategy

- **Status**: Accepted
- **Created**: 2024-05-23
- **Context**: `locald-builder` output handling

## Summary

This RFC defines the strategy for parsing Cloud Native Buildpack (CNB) lifecycle output to provide a "quiet by default" user experience. We rely on the `===> PHASE_NAME` delimiter used by the reference `buildpacks/lifecycle` implementation to filter verbose logs while surfacing high-level progress.

## Motivation

The `locald` design philosophy emphasizes "Respectful Output" (see `docs/design/axioms.md`). Raw buildpack output is often extremely verbose (downloading layers, compilation details) and overwhelms the user. We want to show:

1.  High-level lifecycle phases (Detecting, Building, Exporting).
2.  Only relevant details (e.g., "Installing Node.js 18.x").
3.  Full logs only on failure or when requested (`--verbose`).

## Technical Constraint

The CNB specification does not currently mandate a structured logging format (e.g., JSON) for the lifecycle binary's standard output. However, the reference implementation (`buildpacks/lifecycle`) uses a consistent human-readable format:

```text
===> DETECTING
...
===> ANALYZING
...
===> BUILDING
...
```

## Design

### 1. Heuristic Parsing

We implement a stream parser in `locald-builder` that reads `stdout`/`stderr` from the lifecycle container.

- **Rule**: Lines starting with `===>` are treated as **Phase Headers**.
- **Action**: Phase Headers are logged at `INFO` level.
- **Default**: All other lines are logged at `DEBUG` level (hidden by default).

### 2. Validation Strategy

Since this relies on "screen scraping" a CLI tool, we must validate it to prevent regression if the lifecycle tool changes its output format.

- **Version Pinning**: We pin the `buildpacks/lifecycle` version in `locald-builder`. Updates to this version must be tested.
- **Regression Testing**: We maintain a regression test (e.g., `tests/regression_output_format.rs`) that runs a minimal build and asserts that:
  1.  The `===>` markers are present.
  2.  The parser correctly identifies them.

### 3. Fallback

If the parser fails to find `===>` markers (e.g., future lifecycle version changes format):

- The system should degrade gracefully.
- **Safe Mode**: If no phases are detected within $N$ seconds or $M$ lines, switch to streaming _all_ output to ensure the user is not staring at a blank screen during a long build. (Future work).

## Alternatives Considered

- **JSON Logging**: The lifecycle supports `-log-level debug` but not a structured JSON stream for progress.
- **API Polling**: Docker API does not expose internal buildpack phases.

## References

- `buildpacks/lifecycle` implementation of `cmd/lifecycle/main.go` (or equivalent logger).
- CNB Platform Spec (does not specify log format).
