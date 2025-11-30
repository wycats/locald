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

echo "=== Progress (Changelog) ==="
if [ -f "docs/agent-context/changelog.md" ]; then
    cat docs/agent-context/changelog.md
else
    echo "No changelog found."
fi
echo ""

echo "=== Current Phase State ==="
echo "--- Implementation Plan ---"
if [ -f "docs/agent-context/current/implementation-plan.md" ]; then
    cat docs/agent-context/current/implementation-plan.md
else
    echo "(Empty or missing)"
fi
echo ""

echo "--- Task List ---"
if [ -f "docs/agent-context/current/task-list.md" ]; then
    cat docs/agent-context/current/task-list.md
else
    echo "(Empty or missing)"
fi
echo ""

echo "--- Walkthrough (Draft) ---"
if [ -f "docs/agent-context/current/walkthrough.md" ]; then
    cat docs/agent-context/current/walkthrough.md
else
    echo "(Empty or missing)"
fi
echo ""

echo "--- Other Context Files ---"
# List files in current/ that are NOT the standard 3
find docs/agent-context/current -maxdepth 1 -type f \
    ! -name "implementation-plan.md" \
    ! -name "task-list.md" \
    ! -name "walkthrough.md" \
    -exec basename {} \;
echo ""

echo "=== Available Design Docs ==="
if [ -d "docs/design" ]; then
    ls docs/design
else
    echo "No design docs directory found."
fi
