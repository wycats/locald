#!/usr/bin/env bash
set -euo pipefail

# CI tripwire: fail PRs that change Rust src without adding tests.
# Canonical implementation lives in xtask.

cargo xtask ci tripwire
