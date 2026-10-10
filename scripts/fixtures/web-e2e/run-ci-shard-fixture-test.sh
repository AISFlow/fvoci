#!/usr/bin/env bash
# Test-only: exercise run-web-e2e.sh --ci-shard with stubbed builds and run-group.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
FIXTURE_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-web-e2e-fixture.XXXXXX")"
FAKE_BIN="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-web-e2e-fake-bin.XXXXXX")"

cleanup() {
  rm -rf "$FIXTURE_ROOT" "$FAKE_BIN"
}
trap cleanup EXIT

REAL_BUN="$(command -v bun)"
mkdir -p "$FIXTURE_ROOT/scripts" "$FIXTURE_ROOT/apps/web" "$FIXTURE_ROOT/tools/web-e2e"
cp "$ROOT/scripts/run-web-e2e.sh" "$FIXTURE_ROOT/scripts/"
cp "$ROOT/tools/web-e2e/groups.ts" "$ROOT/tools/web-e2e/compat.ts" "$ROOT/tools/web-e2e/run-web-e2e.ts" \
  "$ROOT/tools/web-e2e/shard-fixture.ts" "$FIXTURE_ROOT/tools/web-e2e/"
chmod +x "$FIXTURE_ROOT/scripts/run-web-e2e.sh"

cat >"$FIXTURE_ROOT/scripts/web-e2e-run-group.sh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
# Child processes must not drain the parent's shard plan when stdin is wired incorrectly.
cat >/dev/null || true
echo "fvoci-web-e2e-run-group $*"
exit "${FVOCI_TEST_RUN_GROUP_EXIT:-0}"
STUB
chmod +x "$FIXTURE_ROOT/scripts/web-e2e-run-group.sh"

cat >"$FIXTURE_ROOT/scripts/generate-api.sh" <<'STUB'
#!/usr/bin/env bash
echo "fvoci-web-e2e-fake-generate-api" >&2
STUB
chmod +x "$FIXTURE_ROOT/scripts/generate-api.sh"

# Build-only prerequisite; the separate sqlite-ci fixture checks its failure
# order and exports. This shard fixture never compiles native dependencies.
cat >"$FIXTURE_ROOT/scripts/prepare-sqlite-ci.sh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == --env-file && $# == 2 ]]
echo "fvoci-web-e2e-fake-sqlite-prep" >&2
printf '%s\n' 'export SQLITE3_LIB_DIR=/fixture/sqlite/lib' \
  'export SQLITE3_INCLUDE_DIR=/fixture/sqlite/include' \
  'export SQLITE3_STATIC=1' 'export SQLITE3_NO_PKG_CONFIG=1' >"$2"
STUB

# Only the harness's prepared check and web build are allowed; anything else fails closed.
cat >"$FAKE_BIN/bun" <<'STUB'
#!/usr/bin/env bash
if [[ "$*" == "--bun x --no-install playwright --version" ]]; then
  exit 0
fi
if [[ "$*" == "--bun run build" ]]; then
  echo "fvoci-web-e2e-fake-bun-build" >&2
  if [[ "${FVOCI_TEST_DIRTY_API_DURING_BUILD:-}" == 1 ]]; then
    echo '// dirty during build' >>src/generated/api.ts
  fi
  exit "${FVOCI_TEST_BUN_BUILD_EXIT:-0}"
fi
# The fixture checkout's own group planner runs for real: its plan modes and
# the shard-count query only; so do its plan-line and committed API checks.
if [[ $# -ge 2 && "$1" == @GROUPS@ && ("$2" == verify || "$2" == shard-jsonl || "$2" == shards) ]]; then
  exec @REAL_BUN@ "$@"
fi
if [[ $# -ge 2 && "$1" == @CHECKS@ && ("$2" == plan-specs || "$2" == committed-api) ]]; then
  exec @REAL_BUN@ "$@"
fi
# Test-only handoff leaves: check invocation/propagate refusal, without build.
if [[ $# == 2 && "$1" == @HANDOFF@ && "$2" == admit ]]; then
  [[ "${FVOCI_WEB_BUILD_PHASE:-}" == consume ]] || exit 99
  echo "fvoci-web-e2e-fake-handoff-admit"
  exit 0
fi
if [[ $# == 2 && "$1" == @HANDOFF@ && "$2" == consume ]]; then
  [[ "${FVOCI_WEB_BUILD_PHASE:-}" == consume ]] || exit 99
  echo "fvoci-web-e2e-fake-handoff-consume"
  exit "${FVOCI_TEST_HANDOFF_EXIT:-0}"
fi
echo "unexpected bun invocation: $*" >&2
exit 1
STUB
# The fixture checkout's own handoff path; quoted so it matches only itself.
handoff="$FIXTURE_ROOT/tools/selected-backend-ci/handoff.ts"
sed -i "s|@HANDOFF@|\"${handoff//|/\\|}\"|" "$FAKE_BIN/bun"
groups="$FIXTURE_ROOT/tools/web-e2e/groups.ts"
checks="$FIXTURE_ROOT/tools/web-e2e/run-web-e2e.ts"
sed -i "s|@GROUPS@|\"${groups//|/\\|}\"|; s|@CHECKS@|\"${checks//|/\\|}\"|; s|@REAL_BUN@|\"${REAL_BUN//|/\\|}\"|" "$FAKE_BIN/bun"
chmod +x "$FAKE_BIN/bun"

cat >"$FAKE_BIN/cargo" <<'STUB'
#!/usr/bin/env bash
echo "fvoci-web-e2e-fake-cargo $*" >&2
exit "${FVOCI_TEST_CARGO_EXIT:-0}"
STUB
chmod +x "$FAKE_BIN/cargo"

# Pair names, shard count and timer filters come from groups.ts only.
mapfile -t PAIR_SPECS < <(bun -e '
const { PAIR_FIRST, PAIR_SECOND } = await import(process.argv[1]);
console.log(PAIR_FIRST);
console.log(PAIR_SECOND);
' "$FIXTURE_ROOT/tools/web-e2e/groups.ts")
((${#PAIR_SPECS[@]} == 2))

populate_e2e_tree() {
  local dest="$FIXTURE_ROOT/apps/web/e2e"
  mkdir -p "$dest"
  for name in "${PAIR_SPECS[@]}"; do
    echo "// fixture" >"$dest/$name"
  done
  local i=1
  while (($# > 0)); do
    echo "// fixture" >"$dest/extra-${i}-flow.spec.ts"
    i=$((i + 1))
    shift
  done
}

run_shard() {
  local shard="$1"
  local log
  log="$(mktemp)"
  if ! (
    export PATH="$FAKE_BIN:$PATH"
    export CARGO_TARGET_DIR="$FIXTURE_ROOT/target"
    cd "$FIXTURE_ROOT"
    bash scripts/run-web-e2e.sh --ci-shard "$shard"
  ) >"$log" 2>&1; then
    cat "$log" >&2
    return 1
  fi
  cat "$log"
  rm -f "$log"
}

populate_e2e_tree \
  extra-1-flow.spec.ts extra-2-flow.spec.ts extra-3-flow.spec.ts \
  extra-4-flow.spec.ts extra-5-flow.spec.ts extra-6-flow.spec.ts \
  extra-7-flow.spec.ts extra-8-flow.spec.ts extra-9-flow.spec.ts \
  extra-10-flow.spec.ts extra-11-flow.spec.ts extra-12-flow.spec.ts \
  extra-13-flow.spec.ts extra-14-flow.spec.ts extra-15-flow.spec.ts \
  extra-16-flow.spec.ts extra-17-flow.spec.ts extra-18-flow.spec.ts \
  extra-19-flow.spec.ts extra-20-flow.spec.ts extra-21-flow.spec.ts \
  extra-22-flow.spec.ts extra-23-flow.spec.ts extra-24-flow.spec.ts \
  extra-25-flow.spec.ts extra-26-flow.spec.ts

planned_groups_file="$(mktemp)"
bun -e '
const [groups, e2e] = process.argv.slice(1);
const { DEFAULT_SHARD_COUNT, shardPlanLines } = await import(groups);
for (const line of shardPlanLines(e2e, 0, DEFAULT_SHARD_COUNT)) console.log(line.specs.join(" "));
' "$FIXTURE_ROOT/tools/web-e2e/groups.ts" "$FIXTURE_ROOT/apps/web/e2e" >"$planned_groups_file"

log="$(run_shard 0)"
build_once="$(grep -c 'fvoci-web-e2e-fake-generate-api' <<<"$log" || true)"
mapfile -t executed_groups < <(grep '^fvoci-web-e2e-run-group ' <<<"$log" | sed 's/^fvoci-web-e2e-run-group //')
if [[ "$build_once" -ne 1 ]]; then
  echo "expected exactly one build in --ci-shard fixture run, got ${build_once}" >&2
  exit 1
fi
mapfile -t planned_groups <"$planned_groups_file"
if ((${#executed_groups[@]} != ${#planned_groups[@]})); then
  echo "planned ${#planned_groups[@]} groups but executed ${#executed_groups[@]}" >&2
  exit 1
fi
for i in "${!planned_groups[@]}"; do
  if [[ "${executed_groups[$i]}" != "${planned_groups[$i]}" ]]; then
    echo "group mismatch at ${i}: planned=${planned_groups[$i]} executed=${executed_groups[$i]}" >&2
    exit 1
  fi
done
rm -f "$planned_groups_file"

# Plan failure must happen before build markers.
rm -rf "$FIXTURE_ROOT/apps/web/e2e"
mkdir -p "$FIXTURE_ROOT/apps/web/e2e"
fail_log="$(mktemp)"
if (
  export PATH="$FAKE_BIN:$PATH"
  export CARGO_TARGET_DIR="$FIXTURE_ROOT/target"
  cd "$FIXTURE_ROOT"
  bash scripts/run-web-e2e.sh --ci-shard 0
) >"$fail_log" 2>&1; then
  echo "expected failure for empty e2e tree" >&2
  exit 1
fi
if grep -q 'fvoci-web-e2e-fake-generate-api' "$fail_log"; then
  echo "build ran despite plan failure" >&2
  cat "$fail_log" >&2
  exit 1
fi

# Group failure must fail the shard.
populate_e2e_tree \
  extra-1-flow.spec.ts extra-2-flow.spec.ts extra-3-flow.spec.ts \
  extra-4-flow.spec.ts extra-5-flow.spec.ts extra-6-flow.spec.ts \
  extra-7-flow.spec.ts extra-8-flow.spec.ts extra-9-flow.spec.ts \
  extra-10-flow.spec.ts extra-11-flow.spec.ts extra-12-flow.spec.ts \
  extra-13-flow.spec.ts extra-14-flow.spec.ts extra-15-flow.spec.ts \
  extra-16-flow.spec.ts extra-17-flow.spec.ts extra-18-flow.spec.ts \
  extra-19-flow.spec.ts extra-20-flow.spec.ts extra-21-flow.spec.ts \
  extra-22-flow.spec.ts extra-23-flow.spec.ts extra-24-flow.spec.ts \
  extra-25-flow.spec.ts extra-26-flow.spec.ts
export FVOCI_TEST_RUN_GROUP_EXIT=1
if run_shard 0 >/dev/null; then
  echo "expected shard failure when run-group exits 1" >&2
  exit 1
fi
unset FVOCI_TEST_RUN_GROUP_EXIT

# Shard count override must be rejected.
if (
  export PATH="$FAKE_BIN:$PATH"
  export CARGO_TARGET_DIR="$FIXTURE_ROOT/target"
  export FVOCI_WEB_E2E_SHARD_COUNT=16
  cd "$FIXTURE_ROOT"
  bash scripts/run-web-e2e.sh --ci-shard 0
) >/dev/null 2>&1; then
  echo "expected failure for FVOCI_WEB_E2E_SHARD_COUNT override" >&2
  exit 1
fi


# Explicit CI consumption must use physical tracked outputs from its tested
# checkout, with no generator fallback. All runtime/builds remain stubbed here.
mkdir -p "$FIXTURE_ROOT/apps/web/src/generated"
printf '%s\n' '{"openapi":"3.1.0"}' >"$FIXTURE_ROOT/apps/web/openapi.json"
printf '%s\n' '// committed fixture types' >"$FIXTURE_ROOT/apps/web/src/generated/api.ts"
git init -q "$FIXTURE_ROOT"
git -C "$FIXTURE_ROOT" add scripts apps
fixture_commit() {
  git -C "$FIXTURE_ROOT" -c user.name=Fixture -c user.email=fixture@example.invalid \
    -c commit.gpgsign=false commit -qm "$1"
}
fixture_commit 'Tracked API fixture'
api_log="$FIXTURE_ROOT/committed-api.log"
run_committed_api() {
  (
    export PATH="$FAKE_BIN:$PATH" CARGO_TARGET_DIR="$FIXTURE_ROOT/target"
    export CI="${FVOCI_TEST_CI:-true}" GITHUB_ACTIONS="${FVOCI_TEST_ACTIONS:-true}"
    export GITHUB_JOB="${FVOCI_TEST_JOB:-workspace-browser-shard}"
    export GITHUB_SHA="${FVOCI_TEST_SHA:-$(git -C "$FIXTURE_ROOT" rev-parse HEAD)}"
    cd "$FIXTURE_ROOT"
    export FVOCI_SELECTED_CI_OUTPUT="$FIXTURE_ROOT/browser-output"
    export FVOCI_WEB_BUILD_HANDOFF="$FIXTURE_ROOT/browser-packet"
    export FVOCI_WEB_BUILD_HANDOFF_SHA256="${FVOCI_TEST_HANDOFF_DIGEST-$(printf '%064d' 0)}"
    bash scripts/run-web-e2e.sh --ci-use-committed-api --ci-consume-browser --ci-shard 0 "$@"
  ) >"$api_log" 2>&1
}
reject_committed_api() {
  local label="$1"
  shift
  if run_committed_api "$@"; then
    echo "accepted invalid committed API fixture: $label" >&2
    exit 1
  fi
  if grep -Eq 'fvoci-web-e2e-fake-(generate-api|bun-build|cargo)|^fvoci-web-e2e-run-group ' "$api_log"; then
    echo "preparation/fallback ran after invalid committed API fixture: $label" >&2
    cat "$api_log" >&2
    exit 1
  fi
  echo "committed-api negative: $label rejected"
}

run_committed_api || { cat "$api_log" >&2; exit 1; }
if grep -q 'fvoci-web-e2e-fake-generate-api' "$api_log"; then
  echo 'CI silently regenerated committed outputs' >&2
  exit 1
fi
if grep -Eq 'fvoci-web-e2e-fake-(bun-build|cargo)' "$api_log"; then
  echo 'consumer performed a local build' >&2; exit 1
fi
[[ "$(grep -c 'fvoci-web-e2e-fake-handoff-consume' "$api_log")" == 1 ]]
[[ "$(grep -c '^fvoci-web-e2e-run-group ' "$api_log")" == "${#planned_groups[@]}" ]]
grep -Eq '^web-e2e stage=browser-handoff-consume elapsed_seconds=[0-9]+ exit=0$' "$api_log"
echo 'committed-api positive: same checkout, producer consumption, all groups, zero builds'
FVOCI_TEST_HANDOFF_DIGEST='' reject_committed_api 'missing artifact digest'
status=0
FVOCI_TEST_HANDOFF_EXIT=7 run_committed_api || status=$?
[[ "$status" == 7 ]]
if grep -Eq 'fvoci-web-e2e-fake-(generate-api|bun-build|cargo)|^fvoci-web-e2e-run-group ' "$api_log"; then
  echo 'local fallback/runtime ran after artifact refusal' >&2; exit 1
fi

FVOCI_TEST_CI=false reject_committed_api 'not CI'
FVOCI_TEST_ACTIONS=false reject_committed_api 'not GitHub Actions'
FVOCI_TEST_JOB=web-checks reject_committed_api 'wrong allocated job'
FVOCI_TEST_SHA=0000000000000000000000000000000000000000 reject_committed_api 'wrong tested SHA'
reject_committed_api 'duplicate mode flag' --ci-use-committed-api
printf '%s\n' '// dirty fixture' >>"$FIXTURE_ROOT/apps/web/src/generated/api.ts"
reject_committed_api 'dirty output'
git -C "$FIXTURE_ROOT" add apps/web/src/generated/api.ts
reject_committed_api 'staged dirty output'
git -C "$FIXTURE_ROOT" restore --staged --worktree apps/web/src/generated/api.ts
git -C "$FIXTURE_ROOT" update-index --assume-unchanged apps/web/src/generated/api.ts
printf '%s\n' '// hidden dirty fixture' >>"$FIXTURE_ROOT/apps/web/src/generated/api.ts"
reject_committed_api 'physical dirty output hidden from git diff'
git -C "$FIXTURE_ROOT" update-index --no-assume-unchanged apps/web/src/generated/api.ts
git -C "$FIXTURE_ROOT" restore apps/web/src/generated/api.ts
rm "$FIXTURE_ROOT/apps/web/openapi.json"
reject_committed_api 'missing output'
git -C "$FIXTURE_ROOT" restore apps/web/openapi.json
: >"$FIXTURE_ROOT/apps/web/openapi.json"
reject_committed_api 'empty output'
git -C "$FIXTURE_ROOT" restore apps/web/openapi.json
mv "$FIXTURE_ROOT/apps/web/openapi.json" "$FIXTURE_ROOT/copied-openapi.json"
ln -s ../../copied-openapi.json "$FIXTURE_ROOT/apps/web/openapi.json"
reject_committed_api 'symlink output'
rm "$FIXTURE_ROOT/apps/web/openapi.json"
git -C "$FIXTURE_ROOT" restore apps/web/openapi.json
git -C "$FIXTURE_ROOT" rm --cached -q apps/web/src/generated/api.ts
reject_committed_api 'missing tracked index output'
fixture_commit 'Output absent from HEAD'
reject_committed_api 'untracked output absent from HEAD'
git -C "$FIXTURE_ROOT" add apps/web/src/generated/api.ts
fixture_commit 'Restore tracked output'
printf '%s\n' '// dirty source' >>"$FIXTURE_ROOT/apps/web/e2e/workspace-flow.spec.ts"
reject_committed_api 'dirty tracked source'
git -C "$FIXTURE_ROOT" restore apps/web/e2e/workspace-flow.spec.ts

# A collaboration lane consumer of a verified handoff prepares nothing native:
# no SQLite prep (the workflow step exported it), no API generation, no cargo.
# It still builds dist once (the packet has none and consume compares hashes)
# and verifies committed outputs at entry and after that build only. The
# consume refusal stops the run before the sudo-owned selected runtime.
consumer_log="$FIXTURE_ROOT/selected-consumer.log"
run_selected_consumer() {
  (
    export PATH="$FAKE_BIN:$PATH" CARGO_TARGET_DIR="$FIXTURE_ROOT/target"
    export CI=true GITHUB_ACTIONS=true GITHUB_JOB=collaboration-flow FVOCI_E2E_PENDING=1
    export GITHUB_SHA="$(git -C "$FIXTURE_ROOT" rev-parse HEAD)"
    export FVOCI_SELECTED_CI_OUTPUT="$FIXTURE_ROOT/selected-output"
    export FVOCI_WEB_BUILD_HANDOFF="$FIXTURE_ROOT/selected-packet"
    export FVOCI_WEB_BUILD_HANDOFF_SHA256="$(printf '%064d' 0)" FVOCI_TEST_HANDOFF_EXIT=7
    cd "$FIXTURE_ROOT"
    bash scripts/run-web-e2e.sh --ci-use-committed-api --ci-consume-selected
  ) >"$consumer_log" 2>&1
}
status=0
SQLITE3_LIB_DIR=/fixture/sqlite/lib SQLITE3_INCLUDE_DIR=/fixture/sqlite/include \
  SQLITE3_STATIC=1 SQLITE3_NO_PKG_CONFIG=1 run_selected_consumer || status=$?
consumer_count() { grep -c -- "$1" "$consumer_log" || true; }
if [[ "$status" != 7 || "$(consumer_count fvoci-web-e2e-fake-sqlite-prep)" != 0 ||
  "$(consumer_count fvoci-web-e2e-fake-generate-api)" != 0 ||
  "$(consumer_count fvoci-web-e2e-fake-cargo)" != 0 ||
  "$(consumer_count fvoci-web-e2e-fake-bun-build)" != 1 ||
  "$(consumer_count 'committed API outputs match')" != 2 ||
  "$(consumer_count fvoci-web-e2e-fake-handoff-admit)" != 1 ||
  "$(consumer_count fvoci-web-e2e-fake-handoff-consume)" != 1 ||
  "$(consumer_count '^fvoci-web-e2e-run-group ')" != 0 ]]; then
  echo "selected consumer did not prepare exactly the dist build (status ${status})" >&2
  cat "$consumer_log" >&2
  exit 1
fi
grep -Eq '^web-e2e stage=selected-handoff-consume finished at=[0-9T:Z-]+ elapsed_seconds=[0-9]+ exit=7$' "$consumer_log"
# Missing or partial workflow SQLite environments are refused before building.
for dropped in SQLITE3_LIB_DIR SQLITE3_STATIC; do
  status=0
  (export SQLITE3_LIB_DIR=/fixture/sqlite/lib SQLITE3_INCLUDE_DIR=/fixture/sqlite/include \
    SQLITE3_STATIC=1 SQLITE3_NO_PKG_CONFIG=1
    unset "$dropped"
    run_selected_consumer) || status=$?
  if [[ "$status" == 0 ]] ||
    ! grep -q 'selected consumer requires the workflow-prepared SQLite environment' "$consumer_log" ||
    grep -Eq 'fvoci-web-e2e-fake-(sqlite-prep|bun-build|cargo|handoff-consume)' "$consumer_log"; then
    echo "selected consumer without ${dropped} was not refused before building" >&2
    cat "$consumer_log" >&2
    exit 1
  fi
done
echo 'selected consumer: no native rebuild, one dist build, refusal propagated'

# Exercise the actual timer dispatch prefix with only its downstream runtime
# stubbed; no DB/browser allocation or alternate production mode is introduced.
timer_dispatch="$FIXTURE_ROOT/scripts/timer-dispatch-fixture.sh"
sed '/^RUN_DIR=/,$d' "$ROOT/scripts/web-e2e-run-group.sh" >"$timer_dispatch"
cat >>"$timer_dispatch" <<'STUB'
bash "$ROOT/scripts/web-e2e-run-group.sh" "$@"
STUB
cat >"$FIXTURE_ROOT/scripts/web-e2e-run-group.sh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
bun "$ROOT/tools/web-e2e/shard-fixture.ts" record-args "$@"
for arg in "$@"; do
  if [[ "$arg" == "${FVOCI_TEST_TIMER_FAIL_FILTER:-unset}" ]]; then
    exit 7
  fi
done
STUB

run_timer_dispatch() {
  ROOT="$FIXTURE_ROOT" CARGO_TARGET_DIR="$FIXTURE_ROOT/target" bash "$timer_dispatch" "$@"
}

timer_log="$FIXTURE_ROOT/timer-dispatch.jsonl"
run_timer_dispatch --workers=1 e2e/v050-task-timer.spec.ts --retries=0 --trace=on >"$timer_log"
# The dispatcher's filters are exactly the groups.ts timer filters, so a stale
# copy in web-e2e-run-group.sh fails here. The four runs keep the original
# argv and split the titles 9+7+1+5 without overlap; the restart group (one
# title) consumes the whole DB graph and retires the group's original server.
bun "$ROOT/tools/web-e2e/shard-fixture.ts" timer-rows "$timer_log" "$ROOT/apps/web/e2e/v050-task-timer.spec.ts"

# Existing explicit filters, mixed specs, shard/list/pending selection and
# option ordering pass through once with every original argument unchanged.
bun "$ROOT/tools/web-e2e/shard-fixture.ts" dispatch-cases "$timer_dispatch" "$FIXTURE_ROOT"

# Use the actual export statement: each independently allocated run supplies
# its own default beneath retained Playwright output; explicit paths survive.
evidence_export="$FIXTURE_ROOT/scripts/evidence-export-fixture.sh"
sed -n '/^export FVOCI_W5_EVIDENCE_DIR=/p' "$ROOT/scripts/web-e2e-run-group.sh" >"$evidence_export"
[[ "$(wc -l <"$evidence_export")" -eq 1 ]]
bun "$ROOT/tools/web-e2e/shard-fixture.ts" evidence-export "$evidence_export" "$FIXTURE_ROOT"

# The real retention function must copy default proof/screenshot files before
# the owning runtime directory is removed, using no DB or browser.
retention_fixture="$FIXTURE_ROOT/scripts/evidence-retention-fixture.sh"
sed -n '/^retain_failure_artifacts() {/,/^}/p' "$ROOT/scripts/web-e2e-run-group.sh" >"$retention_fixture"
bun "$ROOT/tools/web-e2e/shard-fixture.ts" retention "$retention_fixture" "$FIXTURE_ROOT"

echo "run-ci-shard-fixture-test: ok"
