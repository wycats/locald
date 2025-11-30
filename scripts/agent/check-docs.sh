#!/bin/bash

# Check if task-list.md exists and all items are checked
TASK_LIST="docs/agent-context/current/task-list.md"
if [ ! -f "$TASK_LIST" ]; then
    echo "Error: $TASK_LIST not found."
    exit 1
fi

UNCHECKED=$(grep -c "\- \[ \]" "$TASK_LIST")
if [ "$UNCHECKED" -ne 0 ]; then
    echo "Error: $TASK_LIST has $UNCHECKED unchecked items."
    echo "Please complete all tasks before transitioning."
    exit 1
fi

# Check if walkthrough.md exists and is not empty
WALKTHROUGH="docs/agent-context/current/walkthrough.md"
if [ ! -f "$WALKTHROUGH" ]; then
    echo "Error: $WALKTHROUGH not found."
    exit 1
fi

if [ ! -s "$WALKTHROUGH" ]; then
    echo "Error: $WALKTHROUGH is empty."
    exit 1
fi

# Check if walkthrough.md has "Changes" section
if ! grep -q "## Changes" "$WALKTHROUGH"; then
    echo "Error: $WALKTHROUGH missing '## Changes' section."
    exit 1
fi

echo "Documentation checks passed."
exit 0
