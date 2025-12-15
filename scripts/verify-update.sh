#!/bin/bash
set -e

# Ensure we are in the root of the workspace
cd "$(dirname "$0")/.."

# Sandbox name
SANDBOX="update-test"

# Build the CLI initially
echo "Building locald (v1)..."
cargo build -p locald-cli

# Path to the binary
LOCALD="./target/debug/locald"

# Ensure daemon is stopped
echo "Stopping any existing daemon..."
$LOCALD --sandbox=$SANDBOX server shutdown || true
pkill -f "locald --sandbox=$SANDBOX" || true
sleep 1

# Start the server explicitly
echo "Starting locald server..."
$LOCALD --sandbox=$SANDBOX server start &
SERVER_PID=$!

# Wait for it to be ready
echo "Waiting for server..."
for i in {1..50}; do
    if $LOCALD --sandbox=$SANDBOX ping >/dev/null 2>&1; then
        echo "Server is up."
        break
    fi
    sleep 0.1
done

# Get the actual PID from the process list (in case $! is the wrapper)
# Debug: show all locald processes
echo "Debug: ps aux | grep locald"
ps aux | grep locald

REAL_PID=$(pgrep -f "locald.*server start" | tail -n 1)
echo "Initial Server PID: $REAL_PID"

if [ -z "$REAL_PID" ]; then
    echo "Failed to find server PID"
    exit 1
fi

# Force a rebuild with a new timestamp
echo "Touching locald-core/src/lib.rs to force version bump..."
touch locald-core/src/lib.rs
sleep 1 # Ensure timestamp difference

echo "Building locald (v2)..."
cargo build -p locald-cli

# Run 'locald up' - this should detect the version change and restart
echo "Running 'locald up'..."
# We use a dummy project path (current dir is fine as long as it doesn't fail immediately)
# Actually, let's use a valid project to be safe
cd examples/adhoc-test
../../target/debug/locald --sandbox=$SANDBOX up

# Check the new PID
NEW_PID=$(pgrep -f "locald.*server start" | tail -n 1)
echo "New Server PID: $NEW_PID"

if [ -z "$NEW_PID" ]; then
    echo "Error: Server is not running after update."
    exit 1
fi

if [ "$REAL_PID" == "$NEW_PID" ]; then
    echo "Error: Server PID did not change. Auto-restart failed."
    exit 1
else
    echo "Success: Server PID changed ($REAL_PID -> $NEW_PID). Auto-restart worked."
fi

# Cleanup
echo "Cleaning up..."
../../target/debug/locald --sandbox=$SANDBOX server shutdown
