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
SELECTED_BACKENDS=false

while (($# > 0)); do
  case "$1" in
    --with-selected-backends)
      if [[ "$SELECTED_BACKENDS" == true ]]; then
        echo "--with-selected-backends may be supplied only once" >&2
        exit 1
      fi
      SELECTED_BACKENDS=true
      shift
      ;;
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
  if ! (cd "$ROOT/apps/web" && bun --bun x --no-install playwright --version) >/dev/null 2>&1; then
    echo "missing web dependencies or Playwright; run scripts/prepare-web-e2e.sh" >&2
    exit 1
  fi
}

build_current_artifacts() {
  local sqlite_env
  sqlite_env="$(mktemp "${TMPDIR:-/tmp}/fvoci-sqlite-env.XXXXXX")"
  if ! bash "$ROOT/scripts/prepare-sqlite-ci.sh" --env-file "$sqlite_env"; then
    rm -f "$sqlite_env"
    return 1
  fi
  # Only the reviewed helper's verified four exports, written after success.
  # shellcheck disable=SC1090
  source "$sqlite_env"
  rm -f "$sqlite_env"
  bash "$ROOT/scripts/generate-api.sh"

  cd "$ROOT/apps/web"
  bun --bun run build

  cd "$ROOT"
  if [[ "$SELECTED_BACKENDS" == true ]]; then
    python3 "$ROOT/scripts/run-selected-backend-e2e.py" record-before --output "$FVOCI_SELECTED_CI_OUTPUT"
    python3 "$ROOT/scripts/run-selected-backend-e2e.py" stage --output "$FVOCI_SELECTED_CI_OUTPUT" --stage-name main -- \
      cargo build --locked --offline --features db-tests,api-schema --bin fvoci-server --bin fvoci-migrate --bin fvoci-e2e-fixture --message-format=json-render-diagnostics
    python3 "$ROOT/scripts/run-selected-backend-e2e.py" stage --output "$FVOCI_SELECTED_CI_OUTPUT" --stage-name lib -- \
      cargo test --locked --offline --features db-tests,api-schema --lib --no-run --message-format=json-render-diagnostics
    python3 "$ROOT/scripts/run-selected-backend-e2e.py" stage --output "$FVOCI_SELECTED_CI_OUTPUT" --stage-name install -- \
      cargo test --locked --offline --features db-tests,api-schema --test selected_install_lifetime --no-run --message-format=json-render-diagnostics
    CARGO_TARGET_DIR="$COLLAB_ENGINE_TARGET_DIR" python3 "$ROOT/scripts/run-selected-backend-e2e.py" stage --output "$FVOCI_SELECTED_CI_OUTPUT" --stage-name engine -- \
      cargo build --locked --offline --manifest-path "$ROOT/crates/collab-engine/Cargo.toml" --features worker --bin collab-engine --message-format=json-render-diagnostics
    python3 "$ROOT/scripts/run-selected-backend-e2e.py" record-after --output "$FVOCI_SELECTED_CI_OUTPUT"
  else
    cargo build --locked --offline --bin fvoci-e2e-fixture --features db-tests
    cargo build --locked --offline --bin fvoci-server --bin fvoci-migrate
    CARGO_TARGET_DIR="$COLLAB_ENGINE_TARGET_DIR" cargo build --locked --offline \
      --manifest-path "$ROOT/crates/collab-engine/Cargo.toml" --features worker --bin collab-engine
  fi
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
    if ! bash "$ROOT/scripts/web-e2e-run-group.sh" "${specs_line[@]}" </dev/null; then
      echo "shard ${shard_index} failed on group: ${group_label}" >&2
      exit 1
    fi
  done

  echo "=== web e2e shard ${shard_index}: all groups passed ===" >&2
}

if [[ "$SELECTED_BACKENDS" == true ]]; then
  if [[ -n "$CI_SHARD" || "${FVOCI_E2E_PENDING:-}" != 1 || ${#SPEC_ARGS[@]} -ne 0 ]]; then
    echo "--with-selected-backends requires the whole pending suite and cannot combine shard/spec/grep options" >&2
    exit 1
  fi
  : "${FVOCI_SELECTED_CI_OUTPUT:?required private current cohort output}"
  : "${GITHUB_ACTIONS:?selected companion requires its allocated GitHub job}"
fi

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
pending_status=0
bash "$ROOT/scripts/web-e2e-run-group.sh" "${SPEC_ARGS[@]}" || pending_status=$?
selected_status=0
if [[ "$SELECTED_BACKENDS" == true ]]; then
  # Mandatory companion is attempted even after pending failure; keep its first status.
  # Preparation/build and original pending suite keep the existing CI runner UID.
  # Only this job-owned output/native prefix transfers to the1000 runtime actor.
  : "${FVOCI_SELECTED_CI_SQLITE_PARENT:?required exact job-owned SQLite parent}"
  python3 - "$SQLITE3_LIB_DIR" "$FVOCI_SELECTED_CI_SQLITE_PARENT" <<'PY_PARENT'
from pathlib import Path
import sys
assert Path(sys.argv[1]).resolve().is_relative_to(Path(sys.argv[2]).resolve())
PY_PARENT
  sudo chown -R 1000:1000 "$FVOCI_SELECTED_CI_OUTPUT" "$FVOCI_SELECTED_CI_SQLITE_PARENT"
  sudo install -d -o 1000 -g 1000 -m 0700 "$FVOCI_SELECTED_CI_OUTPUT/tmp"
  docker_gid="$(stat -c %g /var/run/docker.sock)"
  sudo --preserve-env=PATH,CI,GITHUB_ACTIONS,GITHUB_SHA,GITHUB_REPOSITORY,GITHUB_RUN_ID,GITHUB_RUN_ATTEMPT,GITHUB_JOB,PLAYWRIGHT_BROWSERS_PATH \
    setpriv --reuid=1000 --regid=1000 --groups="$docker_gid" \
    env TMPDIR="$FVOCI_SELECTED_CI_OUTPUT/tmp" \
      python3 "$ROOT/scripts/run-selected-backend-e2e.py" run --output "$FVOCI_SELECTED_CI_OUTPUT" || selected_status=$?
fi
if [[ "$pending_status" -ne 0 ]]; then exit "$pending_status"; fi
exit "$selected_status"
