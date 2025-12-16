#!/bin/bash
set -e

LIFECYCLE_VERSION="v0.20.19"
OS="linux"
ARCH="x86-64"
URL="https://github.com/buildpacks/lifecycle/releases/download/${LIFECYCLE_VERSION}/lifecycle-${LIFECYCLE_VERSION}+${OS}.${ARCH}.tgz"

mkdir -p lifecycle
if [ ! -f lifecycle/creator ]; then
    echo "Downloading lifecycle ${LIFECYCLE_VERSION}..."
    curl -L -o lifecycle.tgz "$URL"
    tar -xzf lifecycle.tgz -C lifecycle
    rm lifecycle.tgz
fi

echo "Lifecycle ready in ./lifecycle"
