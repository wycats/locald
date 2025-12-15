#!/bin/bash
set -e

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}Updating Cargo.lock to latest compatible versions...${NC}"
cargo update

echo -e "${GREEN}Cargo.lock updated.${NC}"

echo -e "${YELLOW}Checking for major version updates...${NC}"
if command -v cargo-outdated &> /dev/null; then
    cargo outdated --workspace --root-deps-only
else
    echo "cargo-outdated not found. Skipping major version check."
    echo "To install: cargo install cargo-outdated"
    echo "Or manually check Cargo.toml files for outdated versions."
fi
