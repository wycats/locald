---
title: CI Acceleration: Stop-the-Line for Main
stage: 0
feature: Developer Experience
exo:
    tool: exo rfc create
    protocol: 1
---

# RFC 0132: CI Acceleration: Stop-the-Line for Main

## 1. Summary

This RFC proposes a CI strategy that balances speed for PR iteration with correctness guarantees for the main branch. The core idea: PRs get a "fast lane" with minimal checks, while main gets comprehensive validation with "stop-the-line" semantics.

## 2. Motivation

Current CI challenges:

1. **Long PR feedback loops**: Full test suites take 5+ minutes, slowing iteration
2. **Main branch instability**: Occasional regressions slip through when PR checks are too light
3. **Resource waste**: Running full suites on every PR commit is expensive

We need a balanced approach that gives developers fast feedback while maintaining main branch quality.

## 3. Detailed Design

### 3.1 PR Fast Lane

For pull requests, run only:

- **Compile check**: `cargo check --all-targets`
- **Clippy**: Catch common issues fast
- **Affected tests**: Tests for changed crates only
- **Format check**: `cargo fmt --check`

Target: < 2 minutes feedback

### 3.2 Main Branch: Full Validation

On merge to main:

- **Full test suite**: All tests across all platforms
- **Coverage collection**: Track coverage trends
- **Integration tests**: Full e2e scenarios
- **Documentation builds**: Ensure docs compile

### 3.3 Stop-the-Line Semantics

If main branch CI fails:

1. **Immediate notification**: Alert maintainers
2. **Block new merges**: Prevent stacking failures
3. **Priority fix**: Next PR must address the failure

### 3.4 Scheduled Deep Checks

Nightly runs include:

- **Security audit**: `cargo audit`
- **Dependency updates check**: `cargo outdated`
- **Full platform matrix**: All OS variants
- **Performance benchmarks**: Track regressions

## 4. Implementation

### 4.1 Workflow Structure

```yaml
# PR workflow (fast)
on: pull_request
jobs:
  fast-lane:
    # Quick checks only

# Main workflow (thorough)  
on:
  push:
    branches: [main]
jobs:
  full-validation:
    # Complete test matrix
```

### 4.2 Affected Test Detection

Use cargo's built-in dependency graph:

```bash
# Find changed crates
git diff --name-only origin/main | xargs cargo metadata ...

# Run tests for affected crates only
cargo test -p affected-crate-1 -p affected-crate-2
```

## 5. Success Metrics

- PR feedback time: < 2 minutes (p95)
- Main branch green rate: > 98%
- Time to fix main failures: < 2 hours

## 6. Drawbacks

- More complex CI configuration
- Risk of PR-only regressions (mitigated by main checks)
- Requires discipline around stop-the-line

## 7. Alternatives Considered

- **Full checks on every PR**: Too slow for iteration
- **Skip main checks**: Leads to branch instability
- **Manual gating**: Doesn't scale

## 8. Unresolved Questions

- Should we implement PR check caching?
- How do we handle flaky tests?
