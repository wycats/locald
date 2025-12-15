#!/bin/bash
set -e

# Ensure we are in the project root
cd "$(dirname "$0")/.."

# 1. Build Dashboard Assets
echo "🎨 Building Dashboard assets..."
(cd locald-dashboard && pnpm install && pnpm build)

# 2. Build locald
echo "🔨 Building locald..."
cargo build --bin locald

# 2. Prepare E2E environment
echo "📦 Preparing E2E environment..."
cd locald-dashboard-e2e
pnpm install

# 3. Install Playwright browsers (idempotent)
# Note: We omit --with-deps to avoid sudo prompts. 
# If tests fail due to missing libs, run 'pnpm exec playwright install --with-deps' manually.
echo "🎭 Installing Playwright browsers..."
pnpm exec playwright install

# 4. Run tests
echo "🧪 Running tests..."
CI=true pnpm run test
