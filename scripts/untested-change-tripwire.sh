#!/usr/bin/env bash
set -euo pipefail

# Tripwire: fail if Rust src changes occur without any test deltas.
# This is heuristic by design.

ROOT_DIR="$(realpath "$(dirname "$0")/..")"
cd "$ROOT_DIR"

BASE_REF="${1:-origin/main}"

# Ensure BASE_REF exists locally.
if ! git rev-parse --verify "$BASE_REF" >/dev/null 2>&1; then
  git fetch -q origin main
fi

BASE_SHA="$(git rev-parse "$BASE_REF")"
HEAD_SHA="$(git rev-parse HEAD)"

CHANGED_FILES="$(git diff --name-only "$BASE_SHA".."$HEAD_SHA")"

if ! echo "$CHANGED_FILES" | grep -Eq '(^|/)src/.*\.rs$'; then
  echo "[tripwire] No Rust src changes detected."
  exit 0
fi

if echo "$CHANGED_FILES" | grep -Eq '(^|/)tests/.*\.rs$|_test\.rs$'; then
  echo "[tripwire] Test file changes detected (ok)."
  exit 0
fi

# Inline tests count if they appear in the diff.
DIFF_RS="$(git diff -U0 "$BASE_SHA".."$HEAD_SHA" -- '*.rs' || true)"

if echo "$DIFF_RS" | grep -Eq '^[+-].*#\[[^\]]*test[^\]]*\]'; then
  echo "[tripwire] Inline test attribute delta detected (ok)."
  exit 0
fi

if echo "$DIFF_RS" | grep -Eq '^[+-].*#\[cfg\(test\)\]'; then
  echo "[tripwire] Inline cfg(test) delta detected (ok)."
  exit 0
fi

if echo "$DIFF_RS" | grep -Eq '^[+-].*\bmod\s+tests\b'; then
  echo "[tripwire] Inline mod tests delta detected (ok)."
  exit 0
fi

cat <<EOF
[tripwire] Rust src changed, but no test deltas detected.

Changed files:
$CHANGED_FILES

Fix:
- Add/adjust tests (integration tests under ./tests or inline #[test]), OR
- If truly no tests needed, add a small inline rationale in the PR description.
EOF

exit 1
