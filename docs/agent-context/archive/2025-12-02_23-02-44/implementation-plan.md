# Phase 17 Implementation Plan: Linting & Code Quality

**Goal**: Enforce stricter code quality standards to catch reliable errors without being overly pedantic.

## 1. Configure Clippy

- [ ] **Workspace Configuration**:
  - Create `clippy.toml` or configure workspace-level lints in `Cargo.toml` (if supported) or via `.cargo/config.toml`.
  - Enable `clippy::pedantic` but allow subjective lints (e.g., `module_name_repetitions`, `must_use_candidate`).
  - Enforce `clippy::unwrap_used` and `clippy::expect_used` to prevent panics in production code (allow in tests).

## 2. Fix Lints

- [ ] **Run Clippy**: Run `cargo clippy --workspace --all-targets -- -D warnings` and fix errors.
- [ ] **Address Common Issues**:
  - Replace `unwrap()`/`expect()` with proper error handling (`?`, `map_err`, etc.).
  - Fix casting issues (`as` -> `try_from`).
  - Fix lifetime/borrowing issues if any.

## 3. CI Enforcement

- [ ] **Update CI**: Ensure the GitHub Actions workflow runs `cargo clippy` and fails on warnings.
- [ ] **Pre-commit Hook**: Ensure `lefthook.yml` runs clippy on pre-commit (already seems to be the case, verify configuration).

## 4. User Verification

- [ ] **Manual Check**:
  - Run `cargo clippy` locally and ensure it passes without warnings.
  - Verify that `locald` still builds and runs correctly.
