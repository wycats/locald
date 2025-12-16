#!/bin/bash
set -e

echo "NOTE: This script is deprecated."
echo "Container execution verification now uses the libcontainer-based OCI example."
echo "Redirecting to scripts/verify-oci-example.sh..."

exec "$(dirname "$0")/verify-oci-example.sh" "$@"
