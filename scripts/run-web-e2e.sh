#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_TARGET_DIR

STAMP_FILE="$CARGO_TARGET_DIR/.fvoci-web-e2e-stamp"

require_fresh_binaries() {
  if [[ ! -f "$STAMP_FILE" ]]; then
    echo "missing preparation stamp $STAMP_FILE; run scripts/prepare-web-e2e.sh" >&2
    exit 1
  fi
  for binary in fvoci-server fvoci-migrate fvoci-e2e-fixture; do
    if [[ ! -x "$CARGO_TARGET_DIR/debug/$binary" ]]; then
      echo "missing $CARGO_TARGET_DIR/debug/$binary; re-run scripts/prepare-web-e2e.sh" >&2
      exit 1
    fi
  done
  if find "$ROOT/src" "$ROOT/apps/web/src" -type f -newer "$STAMP_FILE" -print -quit | grep -q .; then
    echo "Rust or web source changed since preparation; re-run scripts/prepare-web-e2e.sh" >&2
    exit 1
  fi
}

require_prepared() {
  local missing=0
  if [[ ! -x "$CARGO_TARGET_DIR/debug/fvoci-server" ]]; then
    echo "missing $CARGO_TARGET_DIR/debug/fvoci-server; run scripts/prepare-web-e2e.sh" >&2
    missing=1
  fi
  if [[ ! -x "$CARGO_TARGET_DIR/debug/fvoci-migrate" ]]; then
    echo "missing $CARGO_TARGET_DIR/debug/fvoci-migrate; run scripts/prepare-web-e2e.sh" >&2
    missing=1
  fi
  if [[ ! -x "$CARGO_TARGET_DIR/debug/fvoci-e2e-fixture" ]]; then
    echo "missing $CARGO_TARGET_DIR/debug/fvoci-e2e-fixture; run scripts/prepare-web-e2e.sh" >&2
    missing=1
  fi
  if [[ ! -f "$ROOT/apps/web/dist/index.html" ]]; then
    echo "missing $ROOT/apps/web/dist/index.html; run scripts/prepare-web-e2e.sh" >&2
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

require_prepared
require_fresh_binaries

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
