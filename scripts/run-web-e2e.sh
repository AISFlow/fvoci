#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
# Freeze relative output paths before changing cwd for frontend/build commands.
CARGO_TARGET_DIR="$(python3 -c 'import pathlib,sys; print(pathlib.Path(sys.argv[1]).resolve())' "$CARGO_TARGET_DIR")"
COLLAB_ENGINE_TARGET_DIR="$ROOT/crates/collab-engine/target"
export CARGO_TARGET_DIR
export FVOCI_COLLAB_ENGINE="$COLLAB_ENGINE_TARGET_DIR/debug/collab-engine"

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
  CARGO_TARGET_DIR="$COLLAB_ENGINE_TARGET_DIR" cargo build --locked --offline \
    --manifest-path "$ROOT/crates/collab-engine/Cargo.toml" --features worker --bin collab-engine
}

require_prepared
build_current_artifacts

RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-web-e2e.XXXXXX")"
SERVER_LOG="$RUN_DIR/server.log"
PEPPER='{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}'

retain_failure_artifacts() {
  local retain_dir log dest
  retain_dir="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-collab-e2e-fail.XXXXXX")"
  chmod 700 "$retain_dir"
  if [[ -d "$RUN_DIR/playwright-output" ]] && [[ -n "$(ls -A "$RUN_DIR/playwright-output" 2>/dev/null || true)" ]]; then
    cp -a "$RUN_DIR/playwright-output" "$retain_dir/playwright-output"
  fi
  mkdir -p "$retain_dir/owned-server"
  while IFS= read -r -d '' log; do
    dest="$retain_dir/owned-server/$(basename "$(dirname "$log")").log"
    sed -E \
      -e 's#postgres://[^[:space:]]+#postgres://redacted#g' \
      -e 's#(DATABASE_URL|DATABASE_APP_URL|FVOCI_E2E_ADMIN_DATABASE_URL|TEST_DATABASE_URL)=[^[:space:]]+#\1=redacted#g' \
      "$log" >"$dest"
  done < <(find "$RUN_DIR" -mindepth 2 -name server.log -type f -print0 2>/dev/null || true)
  echo "retained failure artifacts in $retain_dir" >&2
}

cleanup() {
  local status=$?
  if (( status != 0 )); then
    retain_failure_artifacts || true
  fi
  rm -rf "$RUN_DIR"
}
trap cleanup EXIT

bash "$ROOT/scripts/start-test-postgres.sh" \
  env RUN_DIR="$RUN_DIR" SERVER_LOG="$SERVER_LOG" PEPPER="$PEPPER" ROOT="$ROOT" \
    CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
  bash "$ROOT/scripts/web-e2e-inner.sh" "$@"
