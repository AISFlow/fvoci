#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_TARGET_DIR

cd "$ROOT"
cargo fetch

cd "$ROOT/apps/web"
npm ci --no-audit --no-fund
npx playwright install chromium

cd "$ROOT"
bash "$ROOT/scripts/generate-api.sh"

cd "$ROOT/apps/web"
npm run build

cd "$ROOT"
cargo build --locked --offline --bin fvoci-server --bin fvoci-migrate --bin fvoci-e2e-fixture --features db-tests

touch "$CARGO_TARGET_DIR/.fvoci-web-e2e-stamp"

echo "Web e2e preparation complete."
