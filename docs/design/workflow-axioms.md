# Workflow Axioms

These principles guide _how_ we build `locald`, ensuring a high-velocity, high-quality development process.

## 1. Shift-Left Validation (Local First)

**"CI is for Compliance, not Debugging."**

Anything that _can_ go wrong in a local environment _must_ be caught in the local environment. We do not use CI as a remote runner for linting, formatting, or unit tests.

- **Mechanism**: We use `lefthook` to enforce pre-commit (fast checks) and pre-push (heavy checks) hooks.
- **Failure Mode**: If a PR fails in CI due to a lint error or a broken unit test, it is considered a process failure, not just a code failure.
- **Role of CI**: CI is reserved for:
  - Compliance checks.
  - Security scans.
  - AI Code Review.
  - Visual Regression Testing.
  - Cross-platform verification (things you _can't_ easily run locally).

## 2. Regression Testing (The "Never Again" Rule)

**"If it broke once, write a test so it never breaks again."**

Every bug fix must be accompanied by a regression test that reproduces the failure (before the fix) and passes (after the fix).

- **Mechanism**:
  - **Unit Tests**: For logic errors within a single module.
  - **Integration Tests**: For interaction bugs (e.g., CLI commands, file system handling).
  - **Fixtures**: Create minimal reproduction cases in `tests/fixtures/` or `examples/` if necessary.
- **Verification**: The test must be run as part of the standard test suite (`cargo test` or `run-test.sh`).

## 3. Tooling Integrity

**"The tools work for us; we don't work for the tools."**

The development environment must be robust, self-correcting, and honest.

- **The "Universal Fix"**: If CI complains, `scripts/fix` must silence it. This script is the single entry point for all automated remediation (Rust `clippy`, Prettier, ESLint). A developer should never need to memorize which tool fixes which file type.
- **Single Source of Truth**: Build configurations (e.g., `lefthook.yml`) must not duplicate logic defined in scripts. If a script determines where files go, the CI config must derive from that reality, not hardcode a fragile copy.
- **Full-Stack Rigor**: We do not lower standards for non-Rust code. The dashboard and scripts are mission-critical engineering artifacts. They require strict typing, comprehensive linting, and zero warnings. "It's just a frontend" is not an excuse for `any`.
