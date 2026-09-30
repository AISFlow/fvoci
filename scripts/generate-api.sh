#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_TARGET_DIR

OPENAPI_JSON="$ROOT/apps/web/openapi.json"
GENERATED_TS="$ROOT/apps/web/src/generated/api.ts"
# The web workspace's locked openapi-typescript, run by Bun; never fetched on demand.
openapi_typescript() {
  (cd "$ROOT/apps/web" && bun --bun x --no-install openapi-typescript "$@")
}

mkdir -p "$(dirname "$GENERATED_TS")"

if ! openapi_typescript --version >/dev/null 2>&1; then
  echo "openapi-typescript is not installed; run scripts/prepare-web-e2e.sh or bun ci" >&2
  exit 1
fi

cargo build --locked --offline --bin fvoci-export-openapi --features api-schema
"$CARGO_TARGET_DIR/debug/fvoci-export-openapi" >"$OPENAPI_JSON"

openapi_typescript "$OPENAPI_JSON" -o "$GENERATED_TS"

echo "Generated $OPENAPI_JSON and $GENERATED_TS"
