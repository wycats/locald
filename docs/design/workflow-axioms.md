# Workflow Axioms

These principles guide *how* we build `locald`, ensuring a high-velocity, high-quality development process.

## 1. Shift-Left Validation (Local First)

**"CI is for Compliance, not Debugging."**

Anything that *can* go wrong in a local environment *must* be caught in the local environment. We do not use CI as a remote runner for linting, formatting, or unit tests.

- **Mechanism**: We use `lefthook` to enforce pre-commit (fast checks) and pre-push (heavy checks) hooks.
- **Failure Mode**: If a PR fails in CI due to a lint error or a broken unit test, it is considered a process failure, not just a code failure.
- **Role of CI**: CI is reserved for:
    - Compliance checks.
    - Security scans.
    - AI Code Review.
    - Visual Regression Testing.
    - Cross-platform verification (things you *can't* easily run locally).
