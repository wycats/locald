#!/bin/bash

# Get active RFCs (Stage 2)
# Note: rfc-status tool moved to exosuit repo. Using grep for now.
ACTIVE_RFCS=$(grep -l "^stage: 2" docs/rfcs/*.md 2>/dev/null | wc -l)

if [ "$ACTIVE_RFCS" -eq 0 ]; then
    echo "Note: No active RFCs (Stage 2: Available) found."
    echo "If you are implementing a feature, ensure you have an RFC in Stage 2."
else
    echo "Found $ACTIVE_RFCS active RFC(s)."
fi

# Check if plan-outline.md exists
PLAN="docs/agent-context/plan-outline.md"
if [ ! -f "$PLAN" ]; then
    echo "Warning: $PLAN not found."
fi

echo "Documentation checks passed."
exit 0
