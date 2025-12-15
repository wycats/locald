# Phase 17 Walkthrough: Linting & Code Quality

**Goal**: Enforce stricter code quality standards to catch reliable errors without being overly pedantic.

## Changes

<!-- agent-template start -->
### Code Quality Improvements

- **Strict Linting**: Enforced `clippy::pedantic` (with some exceptions) and denied `unwrap_used`, `expect_used`, and `panic` in the workspace.
- **Fixes**:
  - **`locald-server/src/cert.rs`**: Removed `#[allow(clippy::expect_used)]` and replaced `expect("Lock poisoned")` with proper error handling (logging and returning `None`).
  - **`locald-server/src/lib.rs`**: Removed `#[allow(clippy::option_if_let_else)]` and refactored port binding logic to use `if let` and `or_else` chains.
  - **`locald-cli/src/main.rs`**: Removed `#[allow(clippy::collapsible_if)]` and used `let_chains` (stable in Rust 1.91) to collapse nested `if let`.
- **CI**: Verified that GitHub Actions workflow runs `cargo clippy --workspace -- -D warnings`.

### Configuration

- **`Cargo.toml`**: Confirmed workspace-level lint configuration:
  - `pedantic`: warn
  - `unwrap_used`, `expect_used`, `panic`: deny
  - Allowed subjective lints like `module_name_repetitions`.
<!-- agent-template end -->
