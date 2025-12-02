#!/bin/bash
set -e

BRANCH=$(git branch --show-current)
echo "Checking CI status for branch: $BRANCH"

# Get the latest run ID
RUN_ID=$(gh run list --branch "$BRANCH" --limit 1 --json databaseId --jq '.[0].databaseId')

if [ -z "$RUN_ID" ]; then
    echo "No CI runs found for branch $BRANCH"
    exit 1
fi

echo "Latest Run ID: $RUN_ID"

# Check if user wants to watch
if [ "$1" == "--watch" ]; then
    gh run watch "$RUN_ID"
    # After watch finishes, show failures if any
fi

# Get status
STATUS=$(gh run view "$RUN_ID" --json conclusion --jq '.conclusion')

if [ "$STATUS" == "success" ]; then
    echo "✅ CI Passed!"
    exit 0
elif [ "$STATUS" == "failure" ]; then
    echo "❌ CI Failed. Fetching failure logs..."
    echo "----------------------------------------"
    gh run view "$RUN_ID" --log-failed
    exit 1
else
    echo "⏳ CI Status: $STATUS"
    echo "Use --watch to follow progress."
fi
