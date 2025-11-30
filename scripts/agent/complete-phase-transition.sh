#!/bin/bash

if [ -z "$1" ]; then
    echo "Error: Please provide a commit message."
    echo "Usage: $0 \"Commit message\""
    exit 1
fi

echo "=== Committing Changes ==="
git add .
git commit -m "$1"

if [ $? -ne 0 ]; then
    echo "Git commit failed. Aborting."
    exit 1
fi

echo "=== Archiving Current Context ==="
TIMESTAMP=$(date +%Y-%m-%d_%H-%M-%S)
ARCHIVE_DIR="docs/agent-context/archive/$TIMESTAMP"
mkdir -p "$ARCHIVE_DIR"
cp docs/agent-context/current/* "$ARCHIVE_DIR/" 2>/dev/null
echo "Context archived to $ARCHIVE_DIR"

echo "=== Emptying Current Context ==="
# Remove all files in current
rm -f docs/agent-context/current/*
# Recreate standard files
touch docs/agent-context/current/task-list.md
touch docs/agent-context/current/walkthrough.md
touch docs/agent-context/current/implementation-plan.md
echo "Context files emptied and reset."

echo "=== Future Work Context ==="
for file in docs/agent-context/future/*; do
    if [ -f "$file" ]; then
        echo "--- $file ---"
        cat "$file"
        echo ""
    fi
done

echo "========================================================"
echo "NEXT STEPS:"
echo "1. Review the future work and current chat context."
echo "2. Propose a plan for the next phase to the user."
echo "3. Once agreed, update 'docs/agent-context/current/task-list.md' and 'docs/agent-context/current/implementation-plan.md'."
echo "4. Prepare to begin the new phase in a new chat session."
echo "========================================================"
