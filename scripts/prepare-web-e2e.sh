#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_TARGET_DIR

cd "$ROOT"
cargo fetch --locked
cargo fetch --locked --manifest-path "$ROOT/crates/collab-engine/Cargo.toml"

bun ci
cd "$ROOT/apps/web"
bun --bun x --no-install playwright install chromium

echo "Web e2e preparation complete."
