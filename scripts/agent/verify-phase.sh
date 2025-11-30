#!/bin/bash

echo "=== Running Verification ==="
if [ -f "scripts/check" ]; then
    ./scripts/check
else
    echo "Error: scripts/check not found."
    exit 1
fi

if [ $? -ne 0 ]; then
    echo "Verification failed! Fix errors before proceeding."
    exit 1
fi

echo "=== Checking Documentation ==="
./scripts/agent/check-docs.sh
if [ $? -ne 0 ]; then
    echo "Documentation check failed!"
    exit 1
fi

echo "=== Coherence Checkpoint ==="
echo "Please manually verify the following documents for alignment with the code:"
echo "1. [Plan] docs/agent-context/current/implementation-plan.md"
echo "2. [Tasks] docs/agent-context/current/task-list.md"
echo "3. [Walkthrough] docs/agent-context/current/walkthrough.md"
echo "4. [Decisions] docs/agent-context/decisions.md"
echo ""
echo "Check for:"
echo "- Are all completed tasks marked in task-list.md?"
echo "- Does walkthrough.md describe the actual changes made?"
echo "- Are new architectural decisions recorded in decisions.md?"

echo "=== Verification Successful ==="
echo "Next Steps:"
echo "1. **Meta-Review**: Update AGENTS.md if workflow needs improvement."
echo "2. **Coherence Check**: Ensure docs match code."
echo "3. **Walkthrough**: Update docs/agent-context/current/walkthrough.md."
echo "4. Run scripts/agent/prepare-phase-transition.sh when ready to finalize."

