#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
# Freeze relative output paths before changing cwd for frontend/build commands.
CARGO_TARGET_DIR="$(python3 -c 'import pathlib,sys; print(pathlib.Path(sys.argv[1]).resolve())' "$CARGO_TARGET_DIR")"
COLLAB_ENGINE_TARGET_DIR="$ROOT/crates/collab-engine/target"
export CARGO_TARGET_DIR
export FVOCI_COLLAB_ENGINE="$COLLAB_ENGINE_TARGET_DIR/debug/collab-engine"
export ROOT

if [[ -n "${FVOCI_WEB_E2E_DRY_RUN:-}" ]]; then
  echo "FVOCI_WEB_E2E_DRY_RUN is not supported" >&2
  exit 1
fi

# CI matrix runs exactly eight browser shards; do not allow runtime overrides.
CI_SHARD_COUNT=8
CI_SHARD=""
SPEC_ARGS=()

while (($# > 0)); do
  case "$1" in
    --ci-shard)
      CI_SHARD="${2:?--ci-shard requires an index}"
      shift 2
      ;;
    --)
      shift
      SPEC_ARGS+=("$@")
      break
      ;;
    --ci-shard-count)
      echo "--ci-shard-count is not supported; CI uses a fixed shard count of ${CI_SHARD_COUNT}" >&2
      exit 1
      ;;
    *)
      SPEC_ARGS+=("$1")
      shift
      ;;
  esac
done

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

run_ci_shard() {
  local shard_index="$1"

  if [[ -n "${FVOCI_WEB_E2E_SHARD_COUNT:-}" ]]; then
    echo "FVOCI_WEB_E2E_SHARD_COUNT must not override the fixed CI shard count (${CI_SHARD_COUNT})" >&2
    exit 1
  fi
  if (( shard_index < 0 || shard_index >= CI_SHARD_COUNT )); then
    echo "shard index ${shard_index} out of range 0..$((CI_SHARD_COUNT - 1))" >&2
    exit 1
  fi

  local plan_file
  plan_file="$(mktemp "${TMPDIR:-/tmp}/fvoci-web-e2e-plan.XXXXXX")"

  if ! python3 "$ROOT/scripts/web-e2e-groups.py" verify --shards "$CI_SHARD_COUNT" >/dev/null; then
    rm -f "$plan_file"
    exit 1
  fi
  if ! python3 "$ROOT/scripts/web-e2e-groups.py" shard-jsonl \
      --index "$shard_index" --shards "$CI_SHARD_COUNT" >"$plan_file"; then
    rm -f "$plan_file"
    exit 1
  fi
  if [[ ! -s "$plan_file" ]]; then
    echo "shard ${shard_index} plan is empty" >&2
    rm -f "$plan_file"
    exit 1
  fi

  echo "=== web e2e shard ${shard_index}/${CI_SHARD_COUNT}: build once ===" >&2
  build_current_artifacts

  local -a plan_lines=()
  mapfile -t plan_lines <"$plan_file"
  rm -f "$plan_file"
  if ((${#plan_lines[@]} < 1)); then
    echo "shard ${shard_index} plan is empty" >&2
    exit 1
  fi

  local group_json specs_line group_label
  for group_json in "${plan_lines[@]}"; do
    [[ -z "$group_json" ]] && continue
    mapfile -t specs_line < <(
      python3 -c 'import json,sys; print("\n".join(json.loads(sys.argv[1])["specs"]))' "$group_json"
    )
    if ((${#specs_line[@]} < 1)); then
      echo "shard plan group has no specs: ${group_json}" >&2
      exit 1
    fi
    group_label="$(basename "${specs_line[0]%.spec.ts}")"
    if ((${#specs_line[@]} > 1)); then
      group_label="${group_label}+$(basename "${specs_line[1]%.spec.ts}")"
    fi
    if ! bash "$ROOT/scripts/web-e2e-run-group.sh" "${specs_line[@]}"; then
      echo "shard ${shard_index} failed on group: ${group_label}" >&2
      exit 1
    fi
  done

  echo "=== web e2e shard ${shard_index}: all groups passed ===" >&2
}

require_prepared

if [[ -n "$CI_SHARD" ]]; then
  if ((${#SPEC_ARGS[@]} > 0)); then
    echo "--ci-shard cannot be combined with explicit spec arguments" >&2
    exit 1
  fi
  run_ci_shard "$CI_SHARD"
  exit 0
fi

build_current_artifacts
bash "$ROOT/scripts/web-e2e-run-group.sh" "${SPEC_ARGS[@]}"
