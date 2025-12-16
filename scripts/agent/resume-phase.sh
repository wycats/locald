#!/bin/bash

echo "=== Resuming Work ==="
echo "This script restores the context for the current work."
echo "It will print the current project state, including active RFCs."
echo ""

# Reuse restore-context to dump state
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
"$SCRIPT_DIR/restore-context.sh"
