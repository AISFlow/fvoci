#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_TARGET_DIR

cd "$ROOT"
cargo fetch --locked

cd "$ROOT/apps/web"
npm ci --no-audit --no-fund
npx playwright install chromium

echo "Web e2e preparation complete."
