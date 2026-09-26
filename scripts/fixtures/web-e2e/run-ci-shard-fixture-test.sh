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

mkdir -p "$FIXTURE_ROOT/scripts" "$FIXTURE_ROOT/apps/web/node_modules/.bin"
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

cat >"$FIXTURE_ROOT/apps/web/node_modules/.bin/playwright" <<'STUB'
#!/usr/bin/env bash
exit 0
STUB
chmod +x "$FIXTURE_ROOT/apps/web/node_modules/.bin/playwright"

cat >"$FAKE_BIN/npm" <<'STUB'
#!/usr/bin/env bash
if [[ "${1:-}" == "run" && "${2:-}" == "build" ]]; then
  echo "fvoci-web-e2e-fake-npm-build" >&2
  exit 0
fi
echo "unexpected npm invocation: $*" >&2
exit 1
STUB
chmod +x "$FAKE_BIN/npm"

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

echo "run-ci-shard-fixture-test: ok"
