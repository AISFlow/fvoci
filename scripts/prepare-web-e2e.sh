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
if [[ -n "${FVOCI_WEB_CONTAINER_IMAGE:-}" ]]; then
  # The digest-pinned ci-web image already owns Chromium and its system tools.
  bun --bun x --no-install playwright --version
else
  bun --bun x --no-install playwright install chromium
fi

echo "Web e2e preparation complete."
