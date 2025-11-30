#!/bin/bash

# Check if we are in an active phase
if [ ! -s "docs/agent-context/current/task-list.md" ]; then
  echo "Error: No active phase detected (task-list.md is empty)."
  echo "Please use the 'Starting a New Phase' workflow if you are beginning a new phase."
  exit 1
fi

echo "=== Resuming Phase ==="
echo "This script restores the context for an ongoing phase."
echo "It will print the current project state, including the active task list and implementation plan."
echo "Review this output to determine where you left off and what needs to be done next."
echo ""

# Reuse restore-context to dump state
# Get the directory of the current script
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
"$SCRIPT_DIR/restore-context.sh"
