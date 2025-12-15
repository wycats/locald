#!/bin/bash
set -ex

# Setup sandbox
SANDBOX_NAME="adhoc-verify"
export LOCALD_CLI="../../target/debug/locald"

# Build
cargo build

# Clean sandbox
rm -rf ~/.local/share/locald/sandboxes/$SANDBOX_NAME

# Configure sandbox to use unprivileged ports
mkdir -p ~/.local/share/locald/sandboxes/$SANDBOX_NAME/config/locald
echo '[server]
privileged_ports = false
fallback_ports = true
' > ~/.local/share/locald/sandboxes/$SANDBOX_NAME/config/locald/config.toml

# Start server in background
$LOCALD_CLI --sandbox $SANDBOX_NAME server start &
SERVER_PID=$!

# Wait for server
sleep 2

# 1. Test `locald try`
echo "Testing locald try..."
# We can't easily test interactive prompts in script, but we can test the command runs.
# We'll use `expect` or just assume it runs and we skip the prompt (ctrl-c?).
# Actually `try_cmd` waits for process exit.
# If we run a command that exits immediately, it will prompt.
# We can pipe "n" to it?
echo "n" | $LOCALD_CLI --sandbox $SANDBOX_NAME try "echo 'Hello from try'"

# 2. Test `locald try` history
# The previous command should be saved.
LAST_CMD=$(cat ~/.local/share/locald/sandboxes/$SANDBOX_NAME/data/locald/history)
if [ "$LAST_CMD" != "echo 'Hello from try'" ]; then
    echo "History failed. Expected 'echo 'Hello from try'', got '$LAST_CMD'"
    exit 1
fi

# 3. Test `locald add last`
echo "Testing locald add last..."
$LOCALD_CLI --sandbox $SANDBOX_NAME add --name myservice last

# Verify it was added to locald.toml
if ! grep -q "myservice" locald.toml; then
    echo "Service not added to locald.toml"
    exit 1
fi

# 4. Test `locald run`
echo "Testing locald run..."
# We need to start the project first so the service is loaded?
# Actually `GetServiceEnv` needs the service to be in the config.
# `locald add` modifies locald.toml.
# The daemon watches config changes, so it should pick it up.
sleep 1

# Run a command that prints env
OUTPUT=$($LOCALD_CLI --sandbox $SANDBOX_NAME run myservice "echo \$PORT")
# Since myservice is an exec service, it might not have a port assigned until it starts?
# But `GetServiceEnv` calculates it.
# Wait, `GetServiceEnv` calls `get_service_env` which calls `resolve_env`.
# `get_service_env` gets `service.port`.
# If the service is not running, `service.port` might be None.
# Let's check `ProcessManager::apply_config`. It assigns ports.
# So if the daemon reloaded the config, it should have assigned a port (even if not running yet? No, it starts them).
# Wait, `apply_config` starts services.
# So `myservice` should be running.

if [ -z "$OUTPUT" ]; then
    echo "locald run failed to get PORT"
    exit 1
fi
echo "Got PORT: $OUTPUT"

# Cleanup
kill $SERVER_PID
