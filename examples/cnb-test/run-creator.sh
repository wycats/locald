#!/bin/bash
set -e

# Setup directories
mkdir -p layers platform cache oci-output

export CNB_PLATFORM_API=0.12
export CNB_EXPERIMENTAL_MODE=warn

# Use extracted lifecycle
LIFECYCLE_DIR="./builder-data/cnb/lifecycle"
chmod +x $LIFECYCLE_DIR/*

CNB_DIR="./builder-data/cnb"
TARGET_IMAGE="locald-test-image"
CURRENT_UID=$(id -u)
CURRENT_GID=$(id -g)

echo "===> ANALYZING"
$LIFECYCLE_DIR/analyzer \
  -layers ./layers \
  -run $CNB_DIR/run.toml \
  -log-level debug \
  -daemon=false \
  -layout \
  -layout-dir ./oci-output \
  -uid $CURRENT_UID \
  -gid $CURRENT_GID \
  $TARGET_IMAGE

echo "===> DETECTING"
$LIFECYCLE_DIR/detector \
  -app ./app \
  -buildpacks $CNB_DIR/buildpacks \
  -order $CNB_DIR/order.toml \
  -layers ./layers \
  -platform ./platform \
  -log-level debug

echo "===> RESTORING"
$LIFECYCLE_DIR/restorer \
  -layers ./layers \
  -cache-dir ./cache \
  -log-level debug \
  -uid $CURRENT_UID \
  -gid $CURRENT_GID

echo "===> BUILDING"
$LIFECYCLE_DIR/builder \
  -app ./app \
  -buildpacks $CNB_DIR/buildpacks \
  -layers ./layers \
  -platform ./platform \
  -log-level debug

# echo "===> EXPORTING"
# $LIFECYCLE_DIR/exporter \
#   -layers ./layers \
#   -app ./app \
#   -run $CNB_DIR/run.toml \
#   -log-level debug \
#   -daemon=false \
#   -layout \
#   -layout-dir ./oci-output \
#   -uid $CURRENT_UID \
#   -gid $CURRENT_GID \
#   $TARGET_IMAGE

echo "===> LAUNCHING (Verification)"
export CNB_LAYERS_DIR=$(pwd)/layers
export CNB_APP_DIR=$(pwd)/app
$LIFECYCLE_DIR/launcher node -v

echo "Build complete!"
