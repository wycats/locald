---
title: "Dependency Management Policy"
stage: 1 # Accepted
feature: Engineering Excellence
---

# RFC 0068: Dependency Management Policy

## 1. Summary

This RFC establishes a policy and tooling for keeping project dependencies up to date to ensure security, performance, and access to the latest features.

## 2. Motivation

Outdated dependencies can lead to:

- Security vulnerabilities.
- Compatibility issues with newer tools.
- "Bit rot" where the project becomes hard to build on fresh environments.
- Missed performance improvements and bug fixes.

The user has explicitly requested a mechanism to ensure dependencies are updated "as a matter of course".

## 3. Policy

1.  **Regular Updates**: We should run `cargo update` regularly to keep the lockfile fresh with the latest compatible versions.
2.  **Major Version Checks**: We should periodically check for major version upgrades using `cargo outdated` (or similar tools) and upgrade where feasible.
3.  **Documented Exceptions**: If a dependency _cannot_ be updated (due to breaking changes or bugs), the reason must be documented in `Cargo.toml` comments or a dedicated `DEPENDENCY_EXCEPTIONS.md` file.

## 4. Implementation

### 4.1. Tooling

We will create a script `scripts/update-deps.sh` that:

1.  Runs `cargo update` to update `Cargo.lock` to the latest semver-compatible versions.
2.  Checks for major version updates (if `cargo-outdated` is available).

### 4.2. Workflow Integration

- **Agent Protocol**: Update `AGENTS.md` to include dependency hygiene as a guiding principle.
- **CI/CD**: Optionally, we can add a check to CI to warn about outdated dependencies, but for now, we will rely on the script and agent protocol.

## 5. Future Work

- Automate this with a bot (e.g., Renovate or Dependabot) if we move to a hosted environment that supports it.
- Enforce `cargo outdated` checks in CI.
