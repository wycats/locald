#!/bin/bash
set -e

echo "Building locald..."
cargo build -p locald-cli --bin locald

if [ "${LOCALD_SETUP_SHIM:-}" == "1" ]; then
    echo "Installing/repairing privileged locald-shim (requires sudo)..."
    sudo target/debug/locald admin setup
else
    echo "Note: Some container execution tests require a privileged locald-shim."
    echo "If needed, run: sudo target/debug/locald admin setup"
    echo "(or rerun this script with LOCALD_SETUP_SHIM=1)"
fi

echo "Running workspace tests..."
cargo test --workspace --all-features
