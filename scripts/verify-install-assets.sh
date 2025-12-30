#!/usr/bin/env bash
set -euo pipefail

# Verifies that a fresh `cargo install --path crates/locald-cli` results in a `locald`
# binary that serves the embedded dashboard and docs.
#
# This check intentionally builds in a temporary git worktree with
# `locald-dashboard/build` and `locald-docs/dist` removed, to ensure the Rust
# build path actually generates/embeds assets (rather than relying on existing
# local artifacts).

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: missing required command: $1" >&2
    exit 1
  fi
}

require_cmd git
require_cmd cargo
require_cmd pnpm
require_cmd curl

REPO_ROOT="$(git rev-parse --show-toplevel)"
SANDBOX_NAME="verify-assets-$$"
TMP_DIR="$(mktemp -d)"
WT_DIR="$TMP_DIR/worktree"
INSTALL_ROOT="$TMP_DIR/install-root"
LOG_FILE="$TMP_DIR/locald-server.log"

cleanup() {
  # Always try to shut down the daemon, if it started.
  if [ -x "$INSTALL_ROOT/bin/locald" ]; then
    "$INSTALL_ROOT/bin/locald" --sandbox "$SANDBOX_NAME" server shutdown >/dev/null 2>&1 || true
  fi

  # Remove worktree if it exists.
  if [ -d "$WT_DIR" ]; then
    git -C "$REPO_ROOT" worktree remove --force "$WT_DIR" >/dev/null 2>&1 || true
  fi

  rm -rf "$TMP_DIR" || true
}
trap cleanup EXIT

echo "==> Creating temporary worktree"
git -C "$REPO_ROOT" worktree add --detach "$WT_DIR" HEAD >/dev/null

# If the working tree has local changes (common during development), sync the
# current tree state into the detached worktree so this script verifies what
# you're actually testing (including untracked files).
if [ -n "$(git -C "$REPO_ROOT" status --porcelain=v1)" ]; then
  echo "==> Syncing local working tree into worktree"
  tar -C "$REPO_ROOT" --exclude=.git \
    --exclude=**/node_modules \
    --exclude=**/target \
    --exclude=**/dist \
    --exclude=**/build \
    --exclude=**/.svelte-kit \
    --exclude=**/.turbo \
    --exclude=**/.next \
    -cf - . | tar -C "$WT_DIR" -xf -
fi

# Ensure we aren't accidentally reusing prebuilt assets.
rm -rf "$WT_DIR/locald-dashboard/build" "$WT_DIR/locald-docs/dist"

echo "==> Installing locald into temporary prefix"
# --force to avoid interactive prompts if it already exists.
# Use the worktree paths so locald-server/build.rs can locate locald-dashboard and locald-docs.
cargo install --path "$WT_DIR/crates/locald-cli" --locked --root "$INSTALL_ROOT" --force >/dev/null

LOCALD_BIN="$INSTALL_ROOT/bin/locald"

echo "==> Starting locald daemon (sandbox: $SANDBOX_NAME)"
# Use ephemeral ports to avoid requiring privileges.
# Run in background and capture logs.
LOCALD_HTTP_PORT=0 LOCALD_HTTPS_PORT=0 \
  "$LOCALD_BIN" --sandbox "$SANDBOX_NAME" server start >"$LOG_FILE" 2>&1 &

# Wait for the proxy bind log line and extract the port.
PORT=""
for _ in $(seq 1 120); do
  if grep -E "Proxy bound to http://" -n "$LOG_FILE" >/dev/null 2>&1; then
    # Examples:
    #   Proxy bound to http://0.0.0.0:8081
    #   Proxy bound to http://0.0.0.0:12345
    PORT="$(grep -Eo "Proxy bound to http://[^:]+:[0-9]+" "$LOG_FILE" | head -n 1 | sed -E 's/.*:([0-9]+)$/\1/')"
    if [ -n "$PORT" ]; then
      break
    fi
  fi
  sleep 0.25
done

if [ -z "$PORT" ]; then
  echo "error: failed to detect HTTP proxy port from logs" >&2
  echo "---- locald log tail ----" >&2
  tail -n 200 "$LOG_FILE" >&2 || true
  exit 1
fi

echo "==> Detected HTTP port: $PORT"

check_host_200() {
  local host="$1"
  local path="$2"
  local url="http://127.0.0.1:$PORT$path"

  local code
  code="$(curl -sS -o /dev/null -w "%{http_code}" -H "Host: $host" "$url")"
  if [ "$code" != "200" ]; then
    echo "error: expected 200 for Host=$host $path (got $code)" >&2
    echo "---- locald log tail ----" >&2
    tail -n 200 "$LOG_FILE" >&2 || true
    exit 1
  fi
}

echo "==> Checking embedded dashboard"
check_host_200 "locald.localhost" "/"

echo "==> Checking embedded docs"
check_host_200 "docs.localhost" "/"

echo "==> Shutting down daemon"
"$LOCALD_BIN" --sandbox "$SANDBOX_NAME" server shutdown >/dev/null

echo "OK: installed locald serves embedded dashboard + docs"
