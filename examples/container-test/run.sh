#!/bin/bash
set -e

# Start locald in sandbox mode
../../target/debug/locald --sandbox=test up &
PID=$!

# Wait for locald to initialize
sleep 2

# Wait for redis to be ready
echo "Waiting for redis..."
sleep 5

# Check status
../../target/debug/locald --sandbox=test status

# Get the port
PORT=$(../../target/debug/locald --sandbox=test status | grep redis | awk '{print $3}')
echo "Redis running on port $PORT"

# Test connection (requires redis-cli, or we can just check if port is open)
if command -v redis-cli &> /dev/null; then
    redis-cli -p $PORT ping
else
    echo "redis-cli not found, skipping ping test"
fi

# Stop
../../target/debug/locald --sandbox=test stop

# Kill background process if still running
kill $PID || true

echo "Test passed!"
