# Validation (CI-Aligned)

This repo treats **CI as the contract**. If you’re validating a change for review, prefer running the same checks CI runs.

## One command (recommended)

- `./scripts/check`

This runs the Rust checks and the web builds (dashboard + docs).

## Rust (CI parity)

- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `./scripts/ci-rust-checks-local.sh` (fast mode)
- `LOCALD_PREPUSH_FULL=1 ./scripts/ci-rust-checks-local.sh` (CI-like mode; includes sudo + e2e)

## Web (dashboard + docs)

Dashboard:

- `pnpm -C locald-dashboard install --frozen-lockfile`
- `pnpm -C locald-dashboard build`

Docs:

- `pnpm -C locald-docs install --frozen-lockfile`
- `pnpm -C locald-docs build`
