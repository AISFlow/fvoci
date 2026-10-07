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
CI_COMMITTED_API=false
SELECTED_PHASE="whole"

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
      echo "--ci-shard-count is not supported; CI uses a fixed shard count of ${CI_SHARD_COUNT}" >&2
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
  python3 - "$ROOT" "$CI_SHARD" "$SELECTED_BACKENDS" <<'PY_API'
import os
from pathlib import Path
import re
import stat
import subprocess
import sys

root = Path(sys.argv[1])
def fail(message):
    sys.exit("committed API qualification failed: " + message)
def git(*args):
    return subprocess.check_output(["git", "-C", str(root), *args])
if os.environ.get("CI") != "true" or os.environ.get("GITHUB_ACTIONS") != "true":
    fail("requires GitHub CI")
job = "workspace-browser-shard" if sys.argv[2] else "collaboration-flow"
if os.environ.get("GITHUB_JOB") == "collaboration-build" and sys.argv[3] == "true":
    job = "collaboration-build"
if not sys.argv[2] and sys.argv[3] != "true":
    fail("requires a browser shard or selected companion")
if os.environ.get("GITHUB_JOB") != job:
    fail("requires the allocated browser job")
sha = os.environ.get("GITHUB_SHA", "")
if not re.fullmatch(r"[0-9a-f]{40}", sha) or git("rev-parse", "HEAD").decode().strip() != sha:
    fail("checkout HEAD differs from tested SHA")
if Path(git("rev-parse", "--show-toplevel").decode().strip()).resolve() != root:
    fail("wrapper must belong to this checkout")
if subprocess.run(["git", "-C", str(root), "diff", "--quiet", "HEAD", "--"]).returncode:
    fail("tracked checkout is dirty")
for name in ("apps/web/openapi.json", "apps/web/src/generated/api.ts"):
    path = root / name
    entry = git("ls-tree", "HEAD", "--", name).decode().strip()
    if not entry.startswith("100644 blob ") or not entry.endswith("\t" + name):
        fail(name + " must be a tracked regular output at HEAD")
    oid = entry.split()[2]
    if git("ls-files", "--stage", "--", name).decode().strip() != f"100644 {oid} 0\t{name}":
        fail(name + " index differs from HEAD")
    if not path.exists() or path.resolve() != path or not stat.S_ISREG(path.lstat().st_mode):
        fail(name + " must be a physical regular output")
    content = path.read_bytes()
    if not content or content != git("cat-file", "blob", oid):
        fail(name + " physical bytes differ from HEAD")
print("committed API outputs match tested checkout " + sha)
PY_API
}

run_stage() {
  local name="$1" started="$SECONDS" status
  shift
  echo "web-e2e stage=${name} started" >&2
  if "$@"; then status=0; else status=$?; fi
  echo "web-e2e stage=${name} elapsed_seconds=$((SECONDS - started)) exit=${status}" >&2
  return "$status"
}

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
  if [[ "$CI_COMMITTED_API" == true ]]; then
    verify_committed_api
  else
    run_stage api-generation bash "$ROOT/scripts/generate-api.sh"
  fi

  cd "$ROOT/apps/web"
  run_stage web-build bun --bun run build

  cd "$ROOT"
  if [[ "$CI_COMMITTED_API" == true ]]; then verify_committed_api; fi
  if [[ "$SELECTED_PHASE" == consume ]]; then
    run_stage selected-handoff-consume python3 "$ROOT/scripts/selected-backend-ci/web-build-handoff.py" consume
  elif [[ "$SELECTED_BACKENDS" == true ]]; then
    run_stage selected-input-before python3 "$ROOT/scripts/run-selected-backend-e2e.py" record-before --output "$FVOCI_SELECTED_CI_OUTPUT"
    run_stage selected-main python3 "$ROOT/scripts/run-selected-backend-e2e.py" stage --output "$FVOCI_SELECTED_CI_OUTPUT" --stage-name main -- \
      cargo build --locked --offline --features db-tests,api-schema --bin fvoci-server --bin fvoci-migrate --bin fvoci-e2e-fixture --message-format=json-render-diagnostics
    run_stage selected-lib python3 "$ROOT/scripts/run-selected-backend-e2e.py" stage --output "$FVOCI_SELECTED_CI_OUTPUT" --stage-name lib -- \
      cargo test --locked --offline --features db-tests,api-schema --lib --no-run --message-format=json-render-diagnostics
    run_stage selected-install python3 "$ROOT/scripts/run-selected-backend-e2e.py" stage --output "$FVOCI_SELECTED_CI_OUTPUT" --stage-name install -- \
      cargo test --locked --offline --features db-tests,api-schema --test selected_install_lifetime --no-run --message-format=json-render-diagnostics
    CARGO_TARGET_DIR="$COLLAB_ENGINE_TARGET_DIR" run_stage selected-engine python3 "$ROOT/scripts/run-selected-backend-e2e.py" stage --output "$FVOCI_SELECTED_CI_OUTPUT" --stage-name engine -- \
      cargo build --locked --offline --manifest-path "$ROOT/crates/collab-engine/Cargo.toml" --features worker --bin collab-engine --message-format=json-render-diagnostics
    run_stage selected-input-after python3 "$ROOT/scripts/run-selected-backend-e2e.py" record-after --output "$FVOCI_SELECTED_CI_OUTPUT"
  else
    run_stage fixture-build cargo build --locked --offline --bin fvoci-e2e-fixture --features db-tests
    run_stage default-server-build cargo build --locked --offline --bin fvoci-server --bin fvoci-migrate
    CARGO_TARGET_DIR="$COLLAB_ENGINE_TARGET_DIR" run_stage worker-build cargo build --locked --offline \
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
  if [[ "$SELECTED_PHASE" == whole ]]; then
    [[ "${GITHUB_JOB:-}" == collaboration-flow && -z "${FVOCI_WEB_BUILD_PHASE:-}" ]] || { echo "wrong same-job selected authority" >&2; exit 1; }
  else
    export FVOCI_WEB_BUILD_PHASE="$SELECTED_PHASE"
    [[ "$CI_COMMITTED_API" == true && "${CI:-}" == true && "$GITHUB_ACTIONS" == true ]] || { echo "handoff requires explicit GitHub committed API mode" >&2; exit 1; }
    if [[ "$SELECTED_PHASE" == prepare ]]; then
      [[ "${GITHUB_JOB:-}" == collaboration-build ]] || { echo "wrong producer job" >&2; exit 1; }
    else
      [[ "${GITHUB_JOB:-}" == collaboration-flow ]] || { echo "wrong consumer job" >&2; exit 1; }
      : "${FVOCI_WEB_BUILD_HANDOFF:?missing current producer artifact}"
      : "${FVOCI_WEB_BUILD_HANDOFF_SHA256:?missing current producer digest}"
    fi
  fi
fi

if [[ "$CI_COMMITTED_API" == true ]]; then verify_committed_api; fi
if [[ "$SELECTED_PHASE" == consume ]]; then
  run_stage selected-handoff-admit python3 "$ROOT/scripts/selected-backend-ci/web-build-handoff.py" admit
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
if [[ "$SELECTED_PHASE" == prepare ]]; then
  run_stage selected-handoff-export python3 "$ROOT/scripts/selected-backend-ci/web-build-handoff.py" export
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
  python3 - "$SQLITE3_LIB_DIR" "$FVOCI_SELECTED_CI_SQLITE_PARENT" <<'PY_PARENT'
from pathlib import Path
import sys
assert Path(sys.argv[1]).resolve().is_relative_to(Path(sys.argv[2]).resolve())
PY_PARENT
  # Exclusive runner-owned safe output stays outside the transferred prefixes.
  safe_diagnostics="$RUNNER_TEMP/fvoci-selected-diagnostics"
  python3 - "$safe_diagnostics" <<'PY_DIAGNOSTICS'
from pathlib import Path
import os,sys
prefix=Path(sys.argv[1])
assert prefix == Path(os.environ['RUNNER_TEMP']).resolve()/'fvoci-selected-diagnostics'
prefix.mkdir(mode=0o700)  # occupied/symlink/foreign destinations are refused
assert prefix.stat().st_uid == os.getuid() and prefix.stat().st_mode & 0o777 == 0o700
if os.environ.get('GITHUB_OUTPUT'):
    with open(os.environ['GITHUB_OUTPUT'],'a') as output:
        output.write('selected-safe-diagnostics='+str(prefix)+'\n')
PY_DIAGNOSTICS
  runner_uid="$(id -u)"
  runner_gid="$(id -g)"
  docker_gid="$(stat -c %g /var/run/docker.sock)"
  # Qualify only the existing primary read group actually required by current
  # code/input ancestry. Inaccessible files fail before private ownership moves.
  runtime_groups="$(python3 "$ROOT/scripts/run-selected-backend-e2e.py" permissions \
    --output "$FVOCI_SELECTED_CI_OUTPUT" --sqlite-parent "$FVOCI_SELECTED_CI_SQLITE_PARENT" --docker-gid "$docker_gid")"
  export PLAYWRIGHT_BROWSERS_PATH="$FVOCI_SELECTED_CI_OUTPUT/browser"
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
          python3 "$ROOT/scripts/run-selected-backend-e2e.py" config-list --output "$FVOCI_SELECTED_CI_OUTPUT" \
          >"$safe_diagnostics/config-list.stdout.log" 2>"$safe_diagnostics/config-list.stderr.log") || config_list_exit=$?
    selected_status="$config_list_exit"
  fi
  if [[ "$config_list_exit" == not-run || "$config_list_exit" -eq 0 ]]; then
    sudo --preserve-env=PATH,CI,GITHUB_ACTIONS,GITHUB_SHA,GITHUB_REPOSITORY,GITHUB_RUN_ID,GITHUB_RUN_ATTEMPT,GITHUB_JOB,PLAYWRIGHT_BROWSERS_PATH \
      setpriv --reuid=1000 --regid=1000 --groups="$runtime_groups" \
      env TMPDIR="$FVOCI_SELECTED_CI_OUTPUT/tmp" \
        python3 "$ROOT/scripts/run-selected-backend-e2e.py" run --output "$FVOCI_SELECTED_CI_OUTPUT" || selected_status=$?
    launcher_status="$selected_status"
  fi
  # The launcher has returned, but require existing exact resource-retirement
  # witnesses (or proof no runtime began) before changing private data ownership.
  ownership_status=0
  (umask 077
    sudo --preserve-env=PATH,CI,GITHUB_ACTIONS,GITHUB_SHA,GITHUB_REPOSITORY,GITHUB_RUN_ID,GITHUB_RUN_ATTEMPT,GITHUB_JOB \
      setpriv --reuid=1000 --regid=1000 --groups="$runtime_groups" \
      python3 "$ROOT/scripts/run-selected-backend-e2e.py" owner-return --output "$FVOCI_SELECTED_CI_OUTPUT" \
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
  python3 - "$safe_diagnostics" "$launcher_status" "$ownership_status" "$selected_status" "$pending_status" "$config_list_exit" <<'PY_STATUS' || diagnostic_status=$?
from pathlib import Path
import json,os,sys
prefix=Path(sys.argv[1])
assert not prefix.is_symlink() and prefix.stat().st_uid == os.getuid() and prefix.stat().st_mode & 0o777 == 0o700
with (prefix/'launcher-stage.json').open('x') as receipt:
    os.fchmod(receipt.fileno(),0o600)
    values = [None if value == 'not-run' else int(value) for value in sys.argv[2:]]
    json.dump(dict(zip(('actual_launcher_exit','ownership_return_exit','selected_final_exit','pending_exit','config_list_exit'),values)),receipt)
    receipt.write('\n')
PY_STATUS
  if [[ "$diagnostic_status" -ne 0 && "$selected_status" -eq 0 ]]; then selected_status=1; fi
fi
if [[ "$pending_status" -ne 0 ]]; then exit "$pending_status"; fi
exit "$selected_status"
