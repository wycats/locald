#!/bin/bash
set -e

export CNB_LAYERS_DIR="$(pwd)/layers"
export CNB_APP_DIR="$(pwd)/app"
export CNB_PLATFORM_API=0.12

# The launcher expects to be able to read process types from the layers metadata.
# It usually reads <layers>/launch.toml

echo "Running launcher..."
# Using the downloaded launcher
./lifecycle/lifecycle/launcher bash -c "echo 'Node version:' && node -v && echo 'NPM version:' && npm -v"
