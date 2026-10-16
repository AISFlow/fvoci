#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_TARGET_DIR

cd "$ROOT"
cargo fetch --locked
cargo fetch --locked --manifest-path "$ROOT/crates/collab-engine/Cargo.toml"

# Trusted dependency lifecycle scripts may invoke `node`; Bun owns that
# executable name for this command as well as the later web tools.
bun --bun run bash -c 'bun ci'
cd "$ROOT/apps/web"
if [[ -n "${FVOCI_WEB_CONTAINER_IMAGE:-}" ]]; then
  # The digest-pinned ci-web image already owns Chromium and its system tools.
  bun --bun x --no-install playwright --version
else
  bun --bun x --no-install playwright install chromium
fi

echo "Web e2e preparation complete."
