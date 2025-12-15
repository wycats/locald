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

# Verify daemon is down
if pgrep -f "locald server" > /dev/null; then
    echo "Error: Daemon failed to stop."
    ps aux | grep "locald server"
    exit 1
fi
echo "Daemon is stopped."

# Set environment to disable privileged ports for testing
export LOCALD_PRIVILEGED_PORTS=false

# Run 'locald try' - this should auto-start the daemon
echo "Running 'locald try' (should auto-start daemon)..."
cd examples/adhoc-test
../../target/debug/locald try echo "Hello from try"

# Verify daemon is running
if pgrep -f "locald server" > /dev/null; then
    echo "Success: Daemon was auto-started."
else
    echo "Error: Daemon was NOT auto-started."
    echo "Checking for socket file..."
    ls -l /tmp/locald.sock || echo "Socket file not found."
    echo "Process list:"
    ps aux | grep locald
    exit 1
fi

# Run 'locald run' - this should use the running daemon
echo "Running 'locald run'..."
../../target/debug/locald run web echo "Hello from run"

# Stop the daemon
echo "Stopping daemon..."
../../target/debug/locald stop

echo "Verification complete!"
