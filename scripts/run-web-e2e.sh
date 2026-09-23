#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_TARGET_DIR

require_prepared() {
  local missing=0
  if [[ ! -d "$ROOT/apps/web/node_modules" ]]; then
    echo "missing $ROOT/apps/web/node_modules; run scripts/prepare-web-e2e.sh" >&2
    missing=1
  fi
  if [[ ! -x "$ROOT/apps/web/node_modules/.bin/playwright" ]]; then
    echo "missing Playwright install; run scripts/prepare-web-e2e.sh" >&2
    missing=1
  fi
  if (( missing != 0 )); then
    exit 1
  fi
}

build_current_artifacts() {
  bash "$ROOT/scripts/generate-api.sh"

  cd "$ROOT/apps/web"
  npm run build

  cd "$ROOT"
  cargo build --locked --offline --bin fvoci-e2e-fixture --features db-tests
  cargo build --locked --offline --bin fvoci-server --bin fvoci-migrate
}

require_prepared
build_current_artifacts

RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-web-e2e.XXXXXX")"
SERVER_LOG="$RUN_DIR/server.log"
PEPPER='{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}'

cleanup() {
  rm -rf "$RUN_DIR"
}
trap cleanup EXIT

bash "$ROOT/scripts/start-test-postgres.sh" \
  env RUN_DIR="$RUN_DIR" SERVER_LOG="$SERVER_LOG" PEPPER="$PEPPER" ROOT="$ROOT" \
    CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
  bash "$ROOT/scripts/web-e2e-inner.sh"
