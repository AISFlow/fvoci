#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
# Freeze relative output paths before changing cwd for frontend/build commands.
# realpath -m resolves symlinks and keeps missing components (a fresh target).
CARGO_TARGET_DIR="$(realpath -m -- "$CARGO_TARGET_DIR")"
COLLAB_ENGINE_TARGET_DIR="$ROOT/crates/collab-engine/target"
export CARGO_TARGET_DIR
export FVOCI_COLLAB_ENGINE="$COLLAB_ENGINE_TARGET_DIR/debug/collab-engine"
export ROOT

if [[ -n "${FVOCI_WEB_E2E_DRY_RUN:-}" ]]; then
  echo "FVOCI_WEB_E2E_DRY_RUN is not supported" >&2
  exit 1
fi

# The CI browser shard count is tools/web-e2e/groups.ts's (read in
# run_ci_shard); runtime overrides are refused.
CI_SHARD_COUNT=""
CI_SHARD=""
SPEC_ARGS=()
SELECTED_BACKENDS=false
CI_COMMITTED_API=false
SELECTED_PHASE="whole"
BROWSER_PHASE=""

while (($# > 0)); do
  case "$1" in
    --ci-use-committed-api)
      if [[ "$CI_COMMITTED_API" == true ]]; then
        echo "--ci-use-committed-api may be supplied only once" >&2
        exit 1
      fi
      CI_COMMITTED_API=true
      shift
      ;;
    --ci-prepare-browser|--ci-consume-browser)
      [[ -z "$BROWSER_PHASE" ]] || { echo "duplicate browser phase" >&2; exit 1; }
      if [[ "$1" == --ci-prepare-browser ]]; then BROWSER_PHASE=prepare; else BROWSER_PHASE=consume; fi
      shift
      ;;
    --ci-prepare-selected|--ci-consume-selected)
      [[ "$SELECTED_PHASE" == whole && "$SELECTED_BACKENDS" == false ]] || { echo "duplicate/mixed selected phase" >&2; exit 1; }
      SELECTED_BACKENDS=true
      if [[ "$1" == --ci-prepare-selected ]]; then SELECTED_PHASE=prepare; else SELECTED_PHASE=consume; fi
      shift
      ;;
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
      echo "--ci-shard-count is not supported; CI uses the fixed shard count from tools/web-e2e/groups.ts" >&2
      exit 1
      ;;
    *)
      SPEC_ARGS+=("$1")
      shift
      ;;
  esac
done

verify_committed_api() {
  # web-checks generates and diffs these same-checkout files; its mandatory
  # aggregate gate remains responsible for schema freshness. Never regenerate
  # or fall back if this explicit browser-only consumption fails qualification.
  bun "$ROOT/tools/web-e2e/run-web-e2e.ts" committed-api "$ROOT" "$CI_SHARD" "$SELECTED_BACKENDS" "$BROWSER_PHASE"
}

run_stage() {
  local name="$1" started="$SECONDS" status timestamp=""
  shift
  if [[ "${GITHUB_JOB:-}" == collaboration-build || "${GITHUB_JOB:-}" == collaboration-flow ]]; then
    timestamp=" at=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  fi
  echo "web-e2e stage=${name} started${timestamp}" >&2
  if "$@"; then status=0; else status=$?; fi
  if [[ "${GITHUB_JOB:-}" == collaboration-build || "${GITHUB_JOB:-}" == collaboration-flow ]]; then
    timestamp=" finished at=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  fi
  echo "web-e2e stage=${name}${timestamp} elapsed_seconds=$((SECONDS - started)) exit=${status}" >&2
  return "$status"
}

require_prepared() {
  if ! (cd "$ROOT/apps/web" && bun --bun x --no-install playwright --version) >/dev/null 2>&1; then
    echo "missing web dependencies or Playwright; run scripts/prepare-web-e2e.sh" >&2
    exit 1
  fi
}

prepare_sqlite_env() {
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
}

# This script's one owner of each native build command, per feature profile:
# - selected: the collaboration packet. tools/selected-backend-ci/handoff.ts
#   qualify() requires features [api-schema, db-tests] on every server binary,
#   so api-schema here is intentional. Always run under
#   run-selected-backend-e2e.ts stage, which reads the JSON diagnostics.
# - default: local/browser binaries: the fixture with db-tests only, server
#   and migrate with default features. handoff.ts browserStage still repeats
#   this profile for the browser packet (follow-up: take it from here).
# Sets NATIVE_ARGV.
native_build_argv() {
  local profile="$1" stage="$2"
  case "$profile/$stage" in
    selected/main)
      NATIVE_ARGV=(cargo build --locked --offline --features db-tests,api-schema
        --bin fvoci-server --bin fvoci-migrate --bin fvoci-e2e-fixture) ;;
    selected/lib)
      NATIVE_ARGV=(cargo test --locked --offline --features db-tests,api-schema --lib --no-run) ;;
    selected/install)
      NATIVE_ARGV=(cargo test --locked --offline --features db-tests,api-schema
        --test selected_install_lifetime --no-run) ;;
    selected/engine | default/engine)
      NATIVE_ARGV=(cargo build --locked --offline --manifest-path "$ROOT/crates/collab-engine/Cargo.toml"
        --features worker --bin collab-engine) ;;
    default/fixture)
      NATIVE_ARGV=(cargo build --locked --offline --bin fvoci-e2e-fixture --features db-tests) ;;
    default/server)
      NATIVE_ARGV=(cargo build --locked --offline --bin fvoci-server --bin fvoci-migrate) ;;
    *)
      echo "unknown native build ${profile}/${stage}" >&2
      return 1
      ;;
  esac
  if [[ "$profile" == selected ]]; then NATIVE_ARGV+=(--message-format=json-render-diagnostics); fi
}

# run_native_stage LABEL PROFILE STAGE [WRAPPER...]: one native build stage;
# the engine builds into its own target directory.
run_native_stage() {
  local label="$1" profile="$2" stage="$3" target="$CARGO_TARGET_DIR"
  shift 3
  native_build_argv "$profile" "$stage"
  if [[ "$stage" == engine ]]; then target="$COLLAB_ENGINE_TARGET_DIR"; fi
  CARGO_TARGET_DIR="$target" run_stage "$label" "$@" "${NATIVE_ARGV[@]}"
}

build_current_artifacts() {
  if [[ "$SELECTED_PHASE" == consume ]]; then
    # A verified-handoff consumer compiles nothing natively. Its workflow step
    # already prepared and exported the SQLite prefix; handoff consume checks
    # those exports and hashes the prefix against the producer's inputs.
    if [[ -z "${SQLITE3_LIB_DIR:-}" || -z "${SQLITE3_INCLUDE_DIR:-}" ||
      "${SQLITE3_STATIC:-}" != 1 || "${SQLITE3_NO_PKG_CONFIG:-}" != 1 ]]; then
      echo "selected consumer requires the workflow-prepared SQLite environment" >&2
      return 1
    fi
  else
    prepare_sqlite_env
  fi
  # Committed API outputs were already verified at entry, before any build.
  if [[ "$CI_COMMITTED_API" != true ]]; then
    run_stage api-generation bash "$ROOT/scripts/generate-api.sh"
  fi

  # Consumers still build dist: the collaboration packet carries no dist, and
  # handoff consume requires this dist to equal the producer's asset hashes.
  cd "$ROOT/apps/web"
  run_stage web-build bun --bun run build

  cd "$ROOT"
  if [[ "$CI_COMMITTED_API" == true ]]; then verify_committed_api; fi
  if [[ "$SELECTED_PHASE" == consume ]]; then
    run_stage selected-handoff-consume bun "$ROOT/tools/selected-backend-ci/handoff.ts" consume
  elif [[ "$SELECTED_BACKENDS" == true ]]; then
    local stage selected=(bun "$ROOT/scripts/run-selected-backend-e2e.ts")
    run_stage selected-input-before "${selected[@]}" record-before --output "$FVOCI_SELECTED_CI_OUTPUT"
    for stage in main lib install engine; do
      run_native_stage "selected-${stage}" selected "$stage" \
        "${selected[@]}" stage --output "$FVOCI_SELECTED_CI_OUTPUT" --stage-name "$stage" --
    done
    run_stage selected-input-after "${selected[@]}" record-after --output "$FVOCI_SELECTED_CI_OUTPUT"
  else
    run_native_stage fixture-build default fixture
    run_native_stage default-server-build default server
    run_native_stage worker-build default engine
  fi
}

run_ci_shard() {
  local shard_index="$1"

  if [[ -n "${FVOCI_WEB_E2E_SHARD_COUNT:-}" ]]; then
    echo "FVOCI_WEB_E2E_SHARD_COUNT must not override the fixed CI shard count from tools/web-e2e/groups.ts" >&2
    exit 1
  fi
  if ! CI_SHARD_COUNT="$(bun "$ROOT/tools/web-e2e/groups.ts" shards)" || [[ ! "$CI_SHARD_COUNT" =~ ^[1-9][0-9]*$ ]]; then
    echo "cannot read the CI shard count from tools/web-e2e/groups.ts" >&2
    exit 1
  fi
  if (( shard_index < 0 || shard_index >= CI_SHARD_COUNT )); then
    echo "shard index ${shard_index} out of range 0..$((CI_SHARD_COUNT - 1))" >&2
    exit 1
  fi

  local plan_file
  plan_file="$(mktemp "${TMPDIR:-/tmp}/fvoci-web-e2e-plan.XXXXXX")"

  if ! bun "$ROOT/tools/web-e2e/groups.ts" verify --shards "$CI_SHARD_COUNT" >/dev/null; then
    rm -f "$plan_file"
    exit 1
  fi
  if ! bun "$ROOT/tools/web-e2e/groups.ts" shard-jsonl \
      --index "$shard_index" --shards "$CI_SHARD_COUNT" >"$plan_file"; then
    rm -f "$plan_file"
    exit 1
  fi
  if [[ ! -s "$plan_file" ]]; then
    echo "shard ${shard_index} plan is empty" >&2
    rm -f "$plan_file"
    exit 1
  fi

  echo "=== web e2e shard ${shard_index}/${CI_SHARD_COUNT}: qualify artifacts ===" >&2
  if [[ "$BROWSER_PHASE" == consume ]]; then
    run_stage browser-handoff-consume bun "$ROOT/tools/selected-backend-ci/handoff.ts" consume
  else
    build_current_artifacts
  fi

  local -a plan_lines=()
  mapfile -t plan_lines <"$plan_file"
  rm -f "$plan_file"
  if ((${#plan_lines[@]} < 1)); then
    echo "shard ${shard_index} plan is empty" >&2
    exit 1
  fi

  local group_json specs_text specs_line group_label
  for group_json in "${plan_lines[@]}"; do
    [[ -z "$group_json" ]] && continue
    # The helper refuses a malformed line or an empty spec list.
    specs_text="$(bun "$ROOT/tools/web-e2e/run-web-e2e.ts" plan-specs "$group_json")" || exit 1
    mapfile -t specs_line <<<"$specs_text"
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

if [[ -n "$BROWSER_PHASE" ]]; then
  [[ "$SELECTED_BACKENDS" == false && "$CI_COMMITTED_API" == true && ${#SPEC_ARGS[@]} -eq 0 ]] || { echo "browser handoff requires committed API and no selected/spec options" >&2; exit 1; }
  [[ "${CI:-}" == true && "${GITHUB_ACTIONS:-}" == true ]] || { echo "browser handoff requires GitHub CI" >&2; exit 1; }
  : "${FVOCI_SELECTED_CI_OUTPUT:?required private browser build output}"
  : "${FVOCI_WEB_BUILD_HANDOFF:?missing browser producer packet path}"
  [[ "${FVOCI_E2E_PROFILE:-debug}" == debug ]] || { echo "browser packet requires debug profile" >&2; exit 1; }
  export FVOCI_WEB_BUILD_PHASE="$BROWSER_PHASE"
  if [[ "$BROWSER_PHASE" == prepare ]]; then
    [[ "${GITHUB_JOB:-}" == workspace-browser-build && -z "$CI_SHARD" ]] || { echo "wrong browser producer job" >&2; exit 1; }
  else
    [[ "${GITHUB_JOB:-}" == workspace-browser-shard && -n "$CI_SHARD" ]] || { echo "wrong browser consumer job" >&2; exit 1; }
    : "${FVOCI_WEB_BUILD_HANDOFF_SHA256:?missing browser producer digest}"
  fi
fi
# Hosted shards must consume an exact producer packet, with no local fallback.
if [[ "${GITHUB_ACTIONS:-}" == true && "${GITHUB_JOB:-}" == workspace-browser-shard && "$BROWSER_PHASE" != consume ]]; then
  echo "hosted browser shard requires --ci-consume-browser" >&2
  exit 1
fi

if [[ "$SELECTED_BACKENDS" == true ]]; then
  if [[ -n "$CI_SHARD" || "${FVOCI_E2E_PENDING:-}" != 1 || ${#SPEC_ARGS[@]} -ne 0 ]]; then
    echo "--with-selected-backends requires the whole pending suite and cannot combine shard/spec/grep options" >&2
    exit 1
  fi
  : "${FVOCI_SELECTED_CI_OUTPUT:?required private current cohort output}"
  : "${GITHUB_ACTIONS:?selected companion requires its allocated GitHub job}"
  if [[ "$SELECTED_PHASE" == whole ]]; then
    case "${GITHUB_JOB:-}" in
      collaboration-flow|collaboration-install-on|collaboration-postgres-on|collaboration-sqlite-on|collaboration-postgres-off|collaboration-sqlite-off) ;;
      *) echo "wrong same-job selected authority" >&2; exit 1 ;;
    esac
    [[ -z "${FVOCI_WEB_BUILD_PHASE:-}" ]] || { echo "wrong same-job selected authority" >&2; exit 1; }
  else
    export FVOCI_WEB_BUILD_PHASE="$SELECTED_PHASE"
    [[ "$CI_COMMITTED_API" == true && "${CI:-}" == true && "$GITHUB_ACTIONS" == true ]] || { echo "handoff requires explicit GitHub committed API mode" >&2; exit 1; }
    if [[ "$SELECTED_PHASE" == prepare ]]; then
      [[ "${GITHUB_JOB:-}" == collaboration-build ]] || { echo "wrong producer job" >&2; exit 1; }
    else
      case "${GITHUB_JOB:-}" in
        collaboration-flow|collaboration-install-on|collaboration-postgres-on|collaboration-sqlite-on|collaboration-postgres-off|collaboration-sqlite-off) ;;
        *) echo "wrong consumer job" >&2; exit 1 ;;
      esac
      : "${FVOCI_WEB_BUILD_HANDOFF:?missing current producer artifact}"
      : "${FVOCI_WEB_BUILD_HANDOFF_SHA256:?missing current producer digest}"
    fi
  fi
fi

if [[ "$CI_COMMITTED_API" == true ]]; then verify_committed_api; fi
if [[ "$SELECTED_PHASE" == consume ]]; then
  run_stage selected-handoff-admit bun "$ROOT/tools/selected-backend-ci/handoff.ts" admit
fi
require_prepared

if [[ "$BROWSER_PHASE" == prepare ]]; then
  [[ ! -e "$ROOT/apps/web/dist" ]] || { echo "browser producer requires absent dist" >&2; exit 1; }
  run_stage browser-input-before bun "$ROOT/tools/selected-backend-ci/handoff.ts" browser-before
  (cd "$ROOT/apps/web" && run_stage web-build bun --bun run build)
  verify_committed_api
  run_stage fixture-build bun "$ROOT/tools/selected-backend-ci/handoff.ts" browser-stage fixture
  run_stage default-server-build bun "$ROOT/tools/selected-backend-ci/handoff.ts" browser-stage default
  CARGO_TARGET_DIR="$COLLAB_ENGINE_TARGET_DIR" run_stage worker-build bun "$ROOT/tools/selected-backend-ci/handoff.ts" browser-stage engine
  run_stage browser-input-after bun "$ROOT/tools/selected-backend-ci/handoff.ts" browser-after
  run_stage browser-handoff-export bun "$ROOT/tools/selected-backend-ci/handoff.ts" export
  exit 0
fi

if [[ -n "$CI_SHARD" ]]; then
  if ((${#SPEC_ARGS[@]} > 0)); then
    echo "--ci-shard cannot be combined with explicit spec arguments" >&2
    exit 1
  fi
  run_ci_shard "$CI_SHARD"
  exit 0
fi

build_current_artifacts
if [[ "$SELECTED_PHASE" == prepare ]]; then
  run_stage selected-handoff-export bun "$ROOT/tools/selected-backend-ci/handoff.ts" export
  exit 0
fi
pending_status=0
bash "$ROOT/scripts/web-e2e-run-group.sh" "${SPEC_ARGS[@]}" || pending_status=$?
selected_status=0
if [[ "$SELECTED_BACKENDS" == true ]]; then
  # Mandatory companion is attempted even after pending failure; keep its first status.
  # Preparation/build and original pending suite keep the existing CI runner UID.
  # Only this job-owned output/native prefix transfers to the1000 runtime actor.
  : "${FVOCI_SELECTED_CI_SQLITE_PARENT:?required exact job-owned SQLite parent}"
  bun "$ROOT/tools/web-e2e/run-web-e2e.ts" path-within "$SQLITE3_LIB_DIR" "$FVOCI_SELECTED_CI_SQLITE_PARENT"
  # Exclusive runner-owned safe output stays outside the transferred prefixes.
  safe_diagnostics="$RUNNER_TEMP/fvoci-selected-diagnostics"
  # Occupied, symlinked or foreign destinations are refused.
  bun "$ROOT/tools/web-e2e/run-web-e2e.ts" safe-diagnostics "$safe_diagnostics"
  runner_uid="$(id -u)"
  runner_gid="$(id -g)"
  docker_gid="$(stat -c %g /var/run/docker.sock)"
  # Qualify only the existing primary read group actually required by current
  # code/input ancestry. Inaccessible files fail before private ownership moves.
  runtime_groups="$(bun "$ROOT/scripts/run-selected-backend-e2e.ts" permissions \
    --output "$FVOCI_SELECTED_CI_OUTPUT" --sqlite-parent "$FVOCI_SELECTED_CI_SQLITE_PARENT" --docker-gid "$docker_gid")"
  export PLAYWRIGHT_BROWSERS_PATH="$FVOCI_SELECTED_CI_OUTPUT/browser"
  if [[ -n "${FVOCI_COLLAB_LANE:-}" && "$FVOCI_COLLAB_LANE" != install/on ]]; then
    : "${FVOCI_CLOSED_INSTALL_RECEIPT:?closed install receipt required}"
    [[ -f "$FVOCI_CLOSED_INSTALL_RECEIPT" && ! -L "$FVOCI_CLOSED_INSTALL_RECEIPT" ]] || {
      echo "closed install receipt missing" >&2
      exit 1
    }
    install -m 0400 "$FVOCI_CLOSED_INSTALL_RECEIPT" "$FVOCI_SELECTED_CI_OUTPUT/closed-install-receipt.json"
    export FVOCI_CLOSED_INSTALL_RECEIPT="$FVOCI_SELECTED_CI_OUTPUT/closed-install-receipt.json"
  fi
  sudo chown -h -R 1000:1000 "$FVOCI_SELECTED_CI_OUTPUT" "$FVOCI_SELECTED_CI_SQLITE_PARENT"
  sudo install -d -o 1000 -g 1000 -m 0700 "$FVOCI_SELECTED_CI_OUTPUT/tmp"
  config_list_exit=not-run
  launcher_status=not-run
  if [[ "$SELECTED_PHASE" == consume ]]; then
    config_list_exit=0
    # Runner-owned exclusive captures survive later owner-return refusal. This
    # leaf receives no DB/key inputs and starts no selected fixtures or lanes.
    (umask 077
      set -o noclobber
      sudo --preserve-env=PATH,CI,GITHUB_ACTIONS,GITHUB_SHA,GITHUB_REPOSITORY,GITHUB_RUN_ID,GITHUB_RUN_ATTEMPT,GITHUB_JOB,FVOCI_WEB_BUILD_PHASE,PLAYWRIGHT_BROWSERS_PATH \
        setpriv --reuid=1000 --regid=1000 --groups="$runtime_groups" \
        env TMPDIR="$FVOCI_SELECTED_CI_OUTPUT/tmp" \
          bun "$ROOT/scripts/run-selected-backend-e2e.ts" config-list --output "$FVOCI_SELECTED_CI_OUTPUT" \
          >"$safe_diagnostics/config-list.stdout.log" 2>"$safe_diagnostics/config-list.stderr.log") || config_list_exit=$?
    selected_status="$config_list_exit"
  fi
  lane_args=()
  if [[ -n "${FVOCI_COLLAB_LANE:-}" ]]; then
    case "$FVOCI_COLLAB_LANE" in
      install/on|postgres/on|sqlite/on|postgres/off|sqlite/off) ;;
      *) echo "unknown collaboration lane" >&2; exit 1 ;;
    esac
    lane_args=(--lane "$FVOCI_COLLAB_LANE")
  fi
  if [[ "$config_list_exit" == not-run || "$config_list_exit" -eq 0 ]]; then
    sudo --preserve-env=PATH,CI,GITHUB_ACTIONS,GITHUB_SHA,GITHUB_REPOSITORY,GITHUB_RUN_ID,GITHUB_RUN_ATTEMPT,GITHUB_JOB,PLAYWRIGHT_BROWSERS_PATH \
      setpriv --reuid=1000 --regid=1000 --groups="$runtime_groups" \
      env TMPDIR="$FVOCI_SELECTED_CI_OUTPUT/tmp" \
        bun "$ROOT/scripts/run-selected-backend-e2e.ts" run --output "$FVOCI_SELECTED_CI_OUTPUT" "${lane_args[@]}" || selected_status=$?
    launcher_status="$selected_status"
  fi
  # The launcher has returned, but require existing exact resource-retirement
  # witnesses (or proof no runtime began) before changing private data ownership.
  ownership_status=0
  (umask 077
    sudo --preserve-env=PATH,CI,GITHUB_ACTIONS,GITHUB_SHA,GITHUB_REPOSITORY,GITHUB_RUN_ID,GITHUB_RUN_ATTEMPT,GITHUB_JOB \
      setpriv --reuid=1000 --regid=1000 --groups="$runtime_groups" \
      bun "$ROOT/scripts/run-selected-backend-e2e.ts" owner-return --output "$FVOCI_SELECTED_CI_OUTPUT" "${lane_args[@]}" \
      >"$safe_diagnostics/ownership-stage.json") || ownership_status=$?
  if [[ "$ownership_status" -eq 0 ]]; then
    if ! sudo chown -h -R "$runner_uid:$runner_gid" "$FVOCI_SELECTED_CI_OUTPUT" "$FVOCI_SELECTED_CI_SQLITE_PARENT"; then
      echo "selected runtime ownership restoration failed" >&2
      if [[ "$selected_status" -eq 0 ]]; then selected_status=1; fi
    else
      # Publish the old allowlist only after the strict closure and owner return.
      if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
        if ! {
          echo 'selected-private-diagnostics<<FVOCI_CLOSED_DIAGNOSTICS'
          for item in '*-stderr.log' '*-stage.json' '*-driver.log' selected-ci-receipt.json \
            handoff-input-before-safe.json handoff-input-after-safe.json handoff-input-current-safe.json handoff-input-delta-safe.json; do
            printf '%s/%s\n' "$FVOCI_SELECTED_CI_OUTPUT" "$item"
          done
          echo FVOCI_CLOSED_DIAGNOSTICS
        } >>"$GITHUB_OUTPUT"; then
          echo "selected diagnostic publication failed" >&2
          if [[ "$selected_status" -eq 0 ]]; then selected_status=1; fi
        fi
      fi
    fi
  else
    echo "selected runtime ownership retained: resource retirement proof incomplete" >&2
    if [[ "$selected_status" -eq 0 ]]; then selected_status=1; fi
  fi
  diagnostic_status=0
  bun "$ROOT/tools/web-e2e/run-web-e2e.ts" launcher-receipt "$safe_diagnostics" "$launcher_status" \
    "$ownership_status" "$selected_status" "$pending_status" "$config_list_exit" || diagnostic_status=$?
  if [[ "$diagnostic_status" -ne 0 && "$selected_status" -eq 0 ]]; then selected_status=1; fi
fi
if [[ "$pending_status" -ne 0 ]]; then exit "$pending_status"; fi
exit "$selected_status"
