#!/bin/bash
set -e

# Ensure we are in the root of the workspace
cd "$(dirname "$0")/.."

# Build the CLI
echo "Building locald..."
cargo build -p locald-cli

# Path to the binary
LOCALD="./target/debug/locald"

# Ensure daemon is stopped
echo "Stopping any existing daemon..."
$LOCALD stop || true
pkill -f "locald server" || true
sleep 1

# Set environment to disable privileged ports for testing
export LOCALD_PRIVILEGED_PORTS=false

# Clean up previous test artifacts
rm -f examples/adhoc-test/locald.toml
rm -rf ~/.local/share/locald/history

# 1. Test 'locald try' (Draft Mode) & Auto-start
echo "Testing 'locald try' & Auto-start..."
cd examples/adhoc-test
../../target/debug/locald try echo "Draft Mode Test"

# Verify daemon is running
if ! pgrep -f "locald server" > /dev/null; then
    echo "Error: Daemon failed to auto-start."
    exit 1
fi

# 2. Test History
echo "Testing History..."
LAST_CMD=$(cat ~/.local/share/locald/history | tail -n 1)
if [ "$LAST_CMD" != "echo Draft Mode Test" ]; then
    echo "Error: History mismatch. Expected 'echo Draft Mode Test', got '$LAST_CMD'"
    exit 1
fi

# 3. Test 'locald add last'
echo "Testing 'locald add last'..."
# We need to simulate input for the service name if it prompts, but 'add last' might not prompt if name is provided?
# 'add last' takes optional name.
../../target/debug/locald add last --name "draft-service"

# Verify locald.toml content
if ! grep -q "draft-service" locald.toml; then
    echo "Error: Service 'draft-service' not found in locald.toml"
    cat locald.toml
    exit 1
fi

if ! grep -q "echo Draft Mode Test" locald.toml; then
    echo "Error: Command not found in locald.toml"
    cat locald.toml
    exit 1
fi

# 4. Test 'locald run' (Task Mode)
echo "Testing 'locald run'..."
../../target/debug/locald run draft-service echo "Task Mode Test"

# Stop daemon
echo "Stopping daemon..."
../../target/debug/locald stop

echo "Phase 33 Verification Complete!"
