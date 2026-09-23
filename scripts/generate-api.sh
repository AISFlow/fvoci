#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_TARGET_DIR

OPENAPI_JSON="$ROOT/apps/web/openapi.json"
GENERATED_TS="$ROOT/apps/web/src/generated/api.ts"
OPENAPI_TYPESCRIPT="$ROOT/apps/web/node_modules/.bin/openapi-typescript"

mkdir -p "$(dirname "$GENERATED_TS")"

if [[ ! -x "$OPENAPI_TYPESCRIPT" ]]; then
  echo "openapi-typescript is not installed; run scripts/prepare-web-e2e.sh or npm ci in apps/web" >&2
  exit 1
fi

cargo build --locked --offline --bin fvoci-export-openapi --features api-schema
"$CARGO_TARGET_DIR/debug/fvoci-export-openapi" >"$OPENAPI_JSON"

"$OPENAPI_TYPESCRIPT" "$OPENAPI_JSON" -o "$GENERATED_TS"

echo "Generated $OPENAPI_JSON and $GENERATED_TS"
