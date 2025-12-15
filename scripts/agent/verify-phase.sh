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
echo "Please manually verify the following:"
echo "1. [RFCs] Are active RFCs (Stage 2) up to date with implementation?"
echo "2. [Decisions] Are new architectural decisions recorded in docs/agent-context/decisions.md?"
echo "3. [Changelog] Is docs/agent-context/changelog.md updated?"
echo ""

echo "=== Verification Successful ==="
echo "Next Steps:"
echo "1. **Meta-Review**: Update AGENTS.md if workflow needs improvement."
echo "2. **Coherence Check**: Ensure docs match code."
echo "3. Run scripts/agent/prepare-phase-transition.sh when ready to finalize."
