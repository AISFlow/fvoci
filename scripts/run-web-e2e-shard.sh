#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
CARGO_TARGET_DIR="$(python3 -c 'import pathlib,sys; print(pathlib.Path(sys.argv[1]).resolve())' "$CARGO_TARGET_DIR")"
COLLAB_ENGINE_TARGET_DIR="$ROOT/crates/collab-engine/target"
export CARGO_TARGET_DIR
export FVOCI_COLLAB_ENGINE="$COLLAB_ENGINE_TARGET_DIR/debug/collab-engine"
export ROOT

SHARD_INDEX="${1:?shard index 0..7 required}"
SHARD_COUNT="${FVOCI_WEB_E2E_SHARD_COUNT:-8}"

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

echo "=== web e2e shard ${SHARD_INDEX}/${SHARD_COUNT}: build once ===" >&2
build_current_artifacts

mapfile -t GROUP_LINES < <(
  python3 "$ROOT/scripts/web-e2e-groups.py" shard --index "$SHARD_INDEX" --shards "$SHARD_COUNT"
)

if ((${#GROUP_LINES[@]} == 0)); then
  echo "shard ${SHARD_INDEX} has no groups" >&2
  exit 1
fi

for line in "${GROUP_LINES[@]}"; do
  # shellcheck disable=SC2206
  specs=($line)
  if ! bash "$ROOT/scripts/web-e2e-run-group.sh" "${specs[@]}"; then
    echo "shard ${SHARD_INDEX} failed on group: ${line}" >&2
    exit 1
  fi
done

echo "=== web e2e shard ${SHARD_INDEX}: all groups passed ===" >&2
