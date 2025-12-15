#!/bin/bash
set -e

# Build the shim
echo "Building locald-shim..."
cargo build -p locald-shim

# Copy shim
echo "Copying shim to target/debug/locald-shim..."
# We copy from the build artifact (which might be in target/debug/deps or target/debug)
# But wait, cargo build -p locald-shim puts it in target/debug/locald-shim
# But it's not setuid.
# We need to chown/chmod it in place?
# No, cargo will overwrite it.
# We should copy it to a stable location?
# But locald expects it as a sibling.
# If we run `cargo run`, the binary is in `target/debug/`.
# So the shim MUST be in `target/debug/locald-shim`.
# So we MUST modify `target/debug/locald-shim`.

# Note: This will be overwritten by the next `cargo build`.
# This script is intended to be run AFTER `cargo build` and BEFORE `cargo test` or `cargo run`.

TARGET_SHIM="target/debug/locald-shim"

if [ ! -f "$TARGET_SHIM" ]; then
    echo "Error: $TARGET_SHIM not found. Did the build fail?"
    exit 1
fi

# Set permissions
echo "Setting up setuid for $TARGET_SHIM (requires sudo)..."
# Restrict execution to the current user's group for security
GROUP=$(id -gn)
sudo chown root:$GROUP "$TARGET_SHIM"
sudo chmod 4750 "$TARGET_SHIM"

echo "Test shim setup complete at $(pwd)/$TARGET_SHIM"
echo "You can now run tests without password prompts."
