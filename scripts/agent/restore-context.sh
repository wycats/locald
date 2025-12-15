#!/bin/bash

echo "=== Project Goals (Plan Outline) ==="
if [ -f "docs/agent-context/plan-outline.md" ]; then
    cat docs/agent-context/plan-outline.md
else
    echo "No plan outline found."
fi
echo ""

echo "=== Architecture & Decisions ==="
if [ -f "docs/agent-context/decisions.md" ]; then
    cat docs/agent-context/decisions.md
else
    echo "No decisions log found."
fi
echo ""

echo "=== Active RFCs (Implementation Context) ==="
# Get list of Stage 2 RFCs
# Note: rfc-status tool moved to exosuit repo. Using grep for now.
RFC_FILES=$(grep -l "^stage: 2" docs/rfcs/*.md 2>/dev/null | xargs -n 1 basename 2>/dev/null)

if [ -z "$RFC_FILES" ]; then
    echo "No active RFCs (Stage 2)."
else
    for rfc in $RFC_FILES; do
        echo "--- $rfc ---"
        if [ -f "docs/rfcs/$rfc" ]; then
             cat "docs/rfcs/$rfc"
        else
             echo "Could not find file for RFC: $rfc"
        fi
        echo ""
    done
fi
echo ""

echo "=== Progress (Changelog) ==="
if [ -f "docs/agent-context/changelog.md" ]; then
    cat docs/agent-context/changelog.md
else
    echo "No changelog found."
fi
echo ""

echo "=== Available Design Docs ==="
if [ -d "docs/design" ]; then
    ls docs/design
else
    echo "No design docs directory found."
fi
