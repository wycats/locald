#!/bin/bash
set -e

# Check if AppArmor is restricting unprivileged user namespaces
if [ "$(sysctl -n kernel.apparmor_restrict_unprivileged_userns 2>/dev/null)" == "1" ]; then
    echo "WARNING: AppArmor is restricting unprivileged user namespaces."
    echo "This is common on Ubuntu 23.10+."
    echo "To run the example, you have two options:"
    echo ""
    echo "Option 1: Create an AppArmor profile for locald-shim (Recommended)"
    echo "  sudo apparmor_parser -r -W locald-shim/apparmor.profile"
    echo ""
    echo "Option 2: Temporarily disable the restriction (Not recommended for production)"
    echo "  sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0"
    echo ""
    echo "Checking if we can proceed..."

    # Try a simple unshare to see if it works now
    if ! unshare -U echo "User NS check passed" >/dev/null 2>&1; then
        echo "ERROR: Unable to create user namespace. Please apply one of the fixes above."
        exit 1
    fi
fi

# Ensure we are in the workspace root
cd "$(dirname "$0")/.."

echo "Building oci-example (and locald)..."
cargo build -p oci-example -p locald-cli --bin locald

if [ "${LOCALD_SETUP_SHIM:-}" == "1" ]; then
    echo "Installing/repairing privileged locald-shim (requires sudo)..."
    sudo target/debug/locald admin setup
else
    echo "Note: The example requires a privileged locald-shim (setuid root)."
    echo "If needed, run: sudo target/debug/locald admin setup"
    echo "(or rerun this script with LOCALD_SETUP_SHIM=1)"
fi

echo "Running oci-example..."
cargo run -p oci-example -- alpine:latest echo "Hello from inside the container!"

echo "Success!"