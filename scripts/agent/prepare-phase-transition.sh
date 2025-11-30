#!/bin/bash

# Ensure documentation is up to date
./scripts/agent/check-docs.sh
if [ $? -ne 0 ]; then
    echo "Documentation check failed! Please fix before preparing transition."
    exit 1
fi

echo "=== Current Phase Context ==="
for file in docs/agent-context/current/*; do
    if [ -f "$file" ]; then
        echo "--- $file ---"
        cat "$file"
        echo ""
    fi
done

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
echo "3. Update 'docs/agent-context/plan-outline.md' to reflect progress."
echo "4. Run 'scripts/agent/complete-phase-transition.sh \"<commit_message>\"' to finalize."
echo "========================================================"
