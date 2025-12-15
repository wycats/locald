#!/bin/bash
set -e

# This script builds the dashboard and docs, and copies them to the server assets directory.
# It is intended to be run before building the locald binary for release/distribution.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVER_ASSETS="$ROOT_DIR/locald-server/src/assets"

echo "Cleaning old assets..."
rm -rf "$SERVER_ASSETS"
mkdir -p "$SERVER_ASSETS"

# --- Build Dashboard ---
echo "Building Dashboard..."
cd "$ROOT_DIR/locald-dashboard"

# We need to use adapter-static for embedding
# But the project is currently configured with adapter-auto.
# For now, let's assume we can build it as a static site.
# If adapter-auto fails to produce a build directory we can use, we might need to swap adapters temporarily or permanently.
# SvelteKit adapter-auto usually builds to .svelte-kit/output/client for static assets if no server logic is used?
# Or we should install adapter-static.

# Let's try to install adapter-static if not present, or just rely on the user having it?
# The user said "firm up the process".
# I should probably modify svelte.config.js to use adapter-static for the build.
# But I don't want to break their dev setup.
# Let's check if we can override the adapter via environment or just install it.

# For now, let's try to build and see what we get.
pnpm install
pnpm build

# SvelteKit with adapter-auto/static usually outputs to `build/` or `.svelte-kit/output/client` depending on config.
# If it's adapter-auto and no specific environment is detected, it might not produce a usable static folder for us easily.
# Let's assume we need to switch to adapter-static for this to work reliably as an embedded app.
# I will modify the svelte.config.js in a separate step if needed.
# For now, let's assume the build output is in `build/` (standard for adapter-static) or check `.svelte-kit`.

if [ -d "build" ]; then
    echo "Copying Dashboard build..."
    cp -r build/* "$SERVER_ASSETS/"
elif [ -d ".svelte-kit/output/client" ]; then
     echo "Copying Dashboard client output..."
     cp -r .svelte-kit/output/client/* "$SERVER_ASSETS/"
     # We also need the index.html which might be in prerendered?
     # adapter-auto is tricky for this.
else
    echo "Error: Could not find dashboard build output."
    exit 1
fi

# --- Build Docs ---
echo "Building Docs..."
cd "$ROOT_DIR/locald-docs"
pnpm install
pnpm build

if [ -d "dist" ]; then
    echo "Copying Docs build..."
    mkdir -p "$SERVER_ASSETS/docs"
    cp -r dist/* "$SERVER_ASSETS/docs/"
else
    echo "Error: Could not find docs build output."
    exit 1
fi

echo "Assets updated successfully in $SERVER_ASSETS"
