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

echo "=== Checking RFC Status ==="
# Check if any RFCs are still in Stage 2
# Note: rfc-status tool moved to exosuit repo. Using grep for now.
ACTIVE_RFCS=$(grep -l "^stage: 2" docs/rfcs/*.md 2>/dev/null | wc -l)

if [ "$ACTIVE_RFCS" -gt 0 ]; then
    echo "Warning: There are still $ACTIVE_RFCS RFCs in Stage 2 (Available)."
    echo "You should update them to Stage 3 (Recommended) if the work is complete."
else
    echo "No active RFCs found. Transition complete."
fi

echo "=== Future Work Context (Stage 0/1 RFCs) ==="
# Show the board
# Note: rfc-status tool moved to exosuit repo. Listing files for now.
ls docs/rfcs/*.md

echo "========================================================"
echo "NEXT STEPS:"
echo "1. Review the future work (Stage 0/1 RFCs)."
echo "2. Select an RFC to work on, or propose a new one (Stage 0)."
echo "3. Move the selected RFC to Stage 2 (Available) to begin implementation."
echo "========================================================"
