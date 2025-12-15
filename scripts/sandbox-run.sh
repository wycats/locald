#!/bin/bash
set -e

# Usage: ./scripts/sandbox-run.sh <sandbox_name> <client_command...>
# Example: ./scripts/sandbox-run.sh my-test start examples/rust-simple

if [ "$#" -lt 2 ]; then
    echo "Usage: $0 <sandbox_name> <client_command...>"
    exit 1
fi

SANDBOX="$1"
shift
CLIENT_ARGS="$@"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT_DIR/target/debug/locald"
LOG_FILE="$ROOT_DIR/server.log"

# 1. Build
echo "📦 Building locald..."
cargo build --bin locald --quiet

# 2. Start Server
echo "🚀 Starting server in sandbox '$SANDBOX'..."
"$BIN" --sandbox="$SANDBOX" server start > "$LOG_FILE" 2>&1 &
SERVER_PID=$!

# 3. Setup Cleanup
cleanup() {
    echo "🛑 Stopping server (PID $SERVER_PID)..."
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
}
trap cleanup EXIT

# 4. Wait for Server
# We loop until the socket exists or we timeout
SOCKET_PATH="$HOME/.local/share/locald/sandboxes/$SANDBOX/locald.sock"
echo "⏳ Waiting for socket at $SOCKET_PATH..."
for i in {1..50}; do
    if [ -S "$SOCKET_PATH" ]; then
        break
    fi
    sleep 0.1
done

if [ ! -S "$SOCKET_PATH" ]; then
    echo "❌ Server failed to start. Check server.log:"
    cat "$LOG_FILE"
    exit 1
fi

# 5. Run Client
echo "Cc Running: locald --sandbox=$SANDBOX $CLIENT_ARGS"
"$BIN" --sandbox="$SANDBOX" $CLIENT_ARGS
