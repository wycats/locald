#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/watch-ci.sh [--run-id ID] [--branch BRANCH] [--workflow NAME] [--interval SECONDS]

Defaults:
  --branch   current git branch
  --workflow CI
  --interval 10

Examples:
  scripts/watch-ci.sh
  scripts/watch-ci.sh --branch phase-24-unified-service-trait
  scripts/watch-ci.sh --run-id 20220012629
EOF
}

RUN_ID=""
BRANCH=""
WORKFLOW="CI"
INTERVAL="10"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --run-id)
      RUN_ID="${2:-}"; shift 2 ;;
    --branch)
      BRANCH="${2:-}"; shift 2 ;;
    --workflow)
      WORKFLOW="${2:-}"; shift 2 ;;
    --interval)
      INTERVAL="${2:-}"; shift 2 ;;
    -h|--help)
      usage; exit 0 ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
done

if ! command -v gh >/dev/null 2>&1; then
  echo "Error: 'gh' CLI not found in PATH" >&2
  exit 127
fi

if [[ -z "$RUN_ID" ]]; then
  if [[ -z "$BRANCH" ]]; then
    if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
      BRANCH="$(git rev-parse --abbrev-ref HEAD)"
    else
      echo "Error: not in a git repo and --branch not provided" >&2
      exit 2
    fi
  fi

  RUN_ID="$(
    gh run list \
      --branch "$BRANCH" \
      --workflow "$WORKFLOW" \
      --limit 1 \
      --json databaseId \
      --jq '.[0].databaseId'
  )"

  if [[ -z "$RUN_ID" || "$RUN_ID" == "null" ]]; then
    echo "Error: no runs found for branch='$BRANCH' workflow='$WORKFLOW'" >&2
    exit 1
  fi
fi

echo "Watching run $RUN_ID (interval=${INTERVAL}s)" >&2

while true; do
  line="$(
    gh run view "$RUN_ID" \
      --json status,conclusion,updatedAt,url,headSha \
      --jq '.status + " " + (.conclusion // "") + " " + .updatedAt + " " + .headSha + " " + .url'
  )"

  status="$(awk '{print $1}' <<<"$line")"
  conclusion="$(awk '{print $2}' <<<"$line")"

  echo "[$(date -Iseconds)] $line" >&2

  if [[ "$status" == "completed" ]]; then
    if [[ "$conclusion" == "success" ]]; then
      exit 0
    fi
    exit 1
  fi

  sleep "$INTERVAL"
done
