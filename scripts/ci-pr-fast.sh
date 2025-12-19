#!/usr/bin/env bash
set -euo pipefail

# Local approximation of the PR fast lane.
# Intentionally excludes coverage and locald-e2e.

ROOT_DIR="$(realpath "$(dirname "$0")/..")"
cd "$ROOT_DIR"

./scripts/untested-change-tripwire.sh

cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings

# Ensure the binaries exist in the target dir that some tests expect.
cargo build -p locald-cli --all-features
cargo build -p locald-shim

cargo test --tests --workspace --all-features --exclude locald-e2e

echo "[ci-pr-fast] OK"
