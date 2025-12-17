#!/usr/bin/env bash
set -euo pipefail

# Phase-level verification entrypoint for `exo verify`.
# Keep this aligned with what CI/pre-commit expects.

cd "$(dirname "${BASH_SOURCE[0]}")/.."

./scripts/check

# Phase 99: keep the acceptance drift-guard and identity tests green.
cargo test -p locald-utils

# Phase 99: ensure the cgroup cleanup e2e test always compiles.
# It self-skips unless prerequisites are met.
cargo test -p locald-e2e --test cgroup_cleanup
