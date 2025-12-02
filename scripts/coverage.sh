#!/bin/bash
set -e

# Check if cargo-llvm-cov is installed
if ! cargo llvm-cov --version &> /dev/null; then
    echo "cargo-llvm-cov is not installed. Installing..."
    cargo install cargo-llvm-cov
fi

echo "Running coverage..."
# Generate html report for local viewing
cargo llvm-cov --all-features --workspace --html

echo "Coverage report generated at target/llvm-cov/html/index.html"
if [[ "$OSTYPE" == "linux-gnu"* ]]; then
    xdg-open target/llvm-cov/html/index.html 2>/dev/null || true
elif [[ "$OSTYPE" == "darwin"* ]]; then
    open target/llvm-cov/html/index.html
fi
