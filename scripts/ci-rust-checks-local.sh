#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(realpath "$(dirname "$0")/..")"

MODE="${LOCALD_PREPUSH_MODE:-fast}"
FULL="${LOCALD_PREPUSH_FULL:-0}"

if [[ "$FULL" == "1" ]]; then
  MODE="full"
fi

TARGET_DIR="${LOCALD_PREPUSH_TARGET_DIR:-"$ROOT_DIR/target/llvm-cov-target"}"
export CARGO_TARGET_DIR="$TARGET_DIR"
export LLVM_PROFILE_FILE="${LLVM_PROFILE_FILE:-"$CARGO_TARGET_DIR/dotlocal-%p-%m.profraw"}"

E2E="${LOCALD_PREPUSH_E2E:-1}"

echo "[pre-push] Rust checks mode: $MODE"
echo "[pre-push] CARGO_TARGET_DIR: $CARGO_TARGET_DIR"

# Keep these aligned with .github/workflows/ci.yml (Rust Checks job), but default
# to a developer-friendly mode (no sudo / no e2e) unless explicitly requested.

cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings

# Ensure the binaries exist in the target dir that tests expect.
cargo build -p locald-cli --all-features
cargo build -p locald-shim

if [[ "$MODE" == "full" ]]; then
  LOCALD_BIN="$CARGO_TARGET_DIR/debug/locald"

  if [[ ! -x "$LOCALD_BIN" ]]; then
    echo "[pre-push] Expected locald binary not found: $LOCALD_BIN" >&2
    exit 1
  fi

  echo "[pre-push] Installing privileged shim (requires sudo)..."
  sudo "$LOCALD_BIN" --sandbox=prepush admin setup
fi

cargo test --tests --workspace --all-features --exclude locald-e2e

if [[ "$MODE" == "full" ]]; then
  LOCALD_BIN="$CARGO_TARGET_DIR/debug/locald"

  echo "[pre-push] Re-installing privileged shim before e2e..."
  sudo "$LOCALD_BIN" --sandbox=prepush admin setup

  if [[ "$E2E" == "1" ]]; then
    cargo test -p locald-e2e --tests --all-features
  else
    echo "[pre-push] Skipping locald-e2e (LOCALD_PREPUSH_E2E=0)"
  fi
fi

echo "[pre-push] Rust checks passed."
