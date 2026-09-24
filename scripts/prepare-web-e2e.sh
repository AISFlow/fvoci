#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_TARGET_DIR

cd "$ROOT"
cargo fetch --locked
cargo fetch --locked --manifest-path "$ROOT/crates/collab-engine/Cargo.toml"

cd "$ROOT/apps/web"
npm ci --no-audit --no-fund
npm ci --prefix "$ROOT/packages/editor" --ignore-scripts --no-audit --no-fund
npx playwright install chromium

echo "Web e2e preparation complete."
