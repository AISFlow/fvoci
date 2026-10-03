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

mkdir -p "$FIXTURE_ROOT/scripts" "$FIXTURE_ROOT/apps/web"
cp "$ROOT/scripts/run-web-e2e.sh" "$ROOT/scripts/web-e2e-groups.py" "$FIXTURE_ROOT/scripts/"
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

# Only the harness's prepared check and web build are allowed; anything else fails closed.
cat >"$FAKE_BIN/bun" <<'STUB'
#!/usr/bin/env bash
if [[ "$*" == "--bun x --no-install playwright --version" ]]; then
  exit 0
fi
if [[ "$*" == "--bun run build" ]]; then
  echo "fvoci-web-e2e-fake-bun-build" >&2
  exit 0
fi
echo "unexpected bun invocation: $*" >&2
exit 1
STUB
chmod +x "$FAKE_BIN/bun"

cat >"$FAKE_BIN/cargo" <<'STUB'
#!/usr/bin/env bash
echo "fvoci-web-e2e-fake-cargo $*" >&2
exit 0
STUB
chmod +x "$FAKE_BIN/cargo"

populate_e2e_tree() {
  local dest="$FIXTURE_ROOT/apps/web/e2e"
  mkdir -p "$dest"
  for name in workspace-flow.spec.ts workspace-wiki-flow.spec.ts; do
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
python3 -c '
import pathlib, sys, importlib.util
root = pathlib.Path(sys.argv[1])
spec = importlib.util.spec_from_file_location("web_e2e_groups", root / "scripts" / "web-e2e-groups.py")
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
e2e = root / "apps" / "web" / "e2e"
for line in mod.shard_plan_lines(e2e, 0, 8):
    print(" ".join(line["specs"]))
' "$FIXTURE_ROOT" >"$planned_groups_file"

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

# Use the actual export statement: each independently allocated run supplies
# its own default beneath retained Playwright output; explicit paths survive.
evidence_export="$FIXTURE_ROOT/scripts/evidence-export-fixture.sh"
sed -n '/^export FVOCI_W5_EVIDENCE_DIR=/p' "$ROOT/scripts/web-e2e-run-group.sh" >"$evidence_export"
[[ "$(wc -l <"$evidence_export")" -eq 1 ]]
python3 - "$evidence_export" "$FIXTURE_ROOT" <<'PYTHON'
import os, pathlib, subprocess, sys
statement, root = sys.argv[1:]
script = 'RUN_DIR="$1"; source "$2"; printf "%s" "$FVOCI_W5_EVIDENCE_DIR"'
clean = dict(os.environ)
clean.pop("FVOCI_W5_EVIDENCE_DIR", None)
for run in ["run-first", "run-second"]:
    directory = str(pathlib.Path(root, run))
    result = subprocess.run(["bash", "-c", script, "fixture", directory, statement], env=clean, text=True, capture_output=True, check=True)
    assert result.stdout == directory + "/playwright-output/w5-evidence", result.stdout
explicit = str(pathlib.Path(root, "caller-owned evidence"))
env = dict(clean, FVOCI_W5_EVIDENCE_DIR=explicit)
result = subprocess.run(["bash", "-c", script, "fixture", root + "/run-third", statement], env=env, text=True, capture_output=True, check=True)
assert result.stdout == explicit, result.stdout
empty = dict(clean, FVOCI_W5_EVIDENCE_DIR="")
result = subprocess.run(["bash", "-c", script, "fixture", root + "/run-empty", statement], env=empty, text=True, capture_output=True, check=True)
assert result.stdout == root + "/run-empty/playwright-output/w5-evidence"
PYTHON

# The real retention function must copy default proof/screenshot files before
# the owning runtime directory is removed, using no DB or browser.
retention_fixture="$FIXTURE_ROOT/scripts/evidence-retention-fixture.sh"
sed -n '/^retain_failure_artifacts() {/,/^}/p' "$ROOT/scripts/web-e2e-run-group.sh" >"$retention_fixture"
python3 - "$retention_fixture" "$FIXTURE_ROOT" <<'PYTHON'
import os, pathlib, subprocess, sys
function, root = sys.argv[1:]
root = pathlib.Path(root)
run = root / "run-retention"
evidence = run / "playwright-output" / "w5-evidence"
evidence.mkdir(parents=True)
files = {"native-proof.json": b'{"fixture":true}', "zoom200.png": b"fixture screenshot bytes"}
for name, value in files.items():
    (evidence / name).write_bytes(value)
temporary = root / "retained-tmp"
temporary.mkdir()
output = root / "retention-github-output"
env = dict(os.environ, RUN_DIR=str(run), SERVER_LOG=str(run / "missing-server.log"), NET_MONITOR_LOG=str(run / "missing-net.log"), NET_MARKS_LOG=str(run / "missing-marks.log"), GROUP_LABEL="timer-evidence-fixture", TMPDIR=str(temporary), GITHUB_OUTPUT=str(output))
script = 'source "$1"; retain_failure_artifacts; rm -rf "$RUN_DIR"'
subprocess.run(["bash", "-eu", "-c", script, "fixture", function], env=env, check=True, capture_output=True)
retained = next(line.split("=", 1)[1] for line in output.read_text().splitlines() if line.startswith("failure-artifacts="))
assert not run.exists()
for name, value in files.items():
    assert (pathlib.Path(retained) / "playwright-output" / "w5-evidence" / name).read_bytes() == value
PYTHON

echo "run-ci-shard-fixture-test: ok"
