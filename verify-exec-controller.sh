#!/bin/bash
set -e

# Define paths
SERVER_BIN="./target/debug/locald"
CLI_BIN="./target/debug/locald"
SERVER_LOG="/tmp/locald-server.log"

# Build binaries
echo "Building binaries..."
cargo build -p locald-cli

# Kill any existing server
pkill -f "locald" || true
sleep 1

# Start server
echo "Starting locald-server..."
RUST_LOG=info $SERVER_BIN server start > $SERVER_LOG 2>&1 &
SERVER_PID=$!
echo "Server PID: $SERVER_PID"

# Wait for server to start
sleep 2

# Add an exec service that prints logs
echo "Adding exec service..."
# Pass the command as a single string so it is correctly interpreted by sh -c
$CLI_BIN service add exec --name test-exec -- "while true; do echo 'Hello from ExecController'; sleep 1; done"

# Wait for service to start and produce logs
sleep 5

# List services
echo "Listing services..."
$CLI_BIN status

# Check service logs
echo "Checking service logs..."
$CLI_BIN logs test-exec > /tmp/service.log
cat /tmp/service.log

if grep -q "Hello from ExecController" /tmp/service.log; then
    echo "SUCCESS: Logs received from ExecController"
else
    echo "FAILURE: Logs NOT received from ExecController"
    exit 1
fi

# Cleanup
echo "Stopping server..."
kill $SERVER_PID
