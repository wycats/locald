#!/bin/bash

# Ensure documentation is up to date
./scripts/agent/check-docs.sh
if [ $? -ne 0 ]; then
    echo "Documentation check failed! Please fix before preparing transition."
    exit 1
fi

echo "=== Active RFCs (To be Completed) ==="
# Note: rfc-status tool moved to exosuit repo. Using grep for now.
RFC_FILES=$(grep -l "^stage: 2" docs/rfcs/*.md 2>/dev/null | xargs -n 1 basename 2>/dev/null)

if [ -z "$RFC_FILES" ]; then
    echo "No active RFCs (Stage 2) found. Nothing to transition?"
else
    for rfc in $RFC_FILES; do
        echo "--- $rfc ---"
        if [ -f "docs/rfcs/$rfc" ]; then
             cat "docs/rfcs/$rfc"
        fi
        echo ""
    done
fi

echo "=== Plan Outline ==="
if [ -f "docs/agent-context/plan-outline.md" ]; then
    echo "--- docs/agent-context/plan-outline.md ---"
    cat docs/agent-context/plan-outline.md
    echo ""
fi

echo "========================================================"
echo "REMINDER:"
echo "1. Update 'docs/agent-context/changelog.md' with completed work."
echo "2. Update 'docs/agent-context/decisions.md' with key decisions."
echo "3. Update the RFC to Stage 3 (Recommended) and consolidate design into 'docs/agent-context/'."
echo "4. Run 'scripts/agent/complete-phase-transition.sh \"<commit_message>\"' to finalize."
echo "========================================================"
