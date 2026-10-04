#!/usr/bin/env bash
# Test-only: collaboration CI runner with stubbed cargo, docker and openssl (the
# real start-test-meili.sh path runs; no container, service or network).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
RUN="$ROOT/scripts/run-rust-collaboration-ci-tests.sh"
FIXTURE_RUN="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-rust-ci-admission-fixture.XXXXXX")"
FAKE_BIN="$FIXTURE_RUN/fake-bin"
INVOCATIONS="$FIXTURE_RUN/cargo-invocations.log"
DOCKER_CALLS="$FIXTURE_RUN/docker-calls.log"

cleanup() {
  rm -rf "$FIXTURE_RUN"
}
trap cleanup EXIT

mkdir -p "$FAKE_BIN"
export RUNNER_TEMP="$FIXTURE_RUN/logs"

# Fixed suite list from main collaboration job (ebca941e); order-independent guard.
EXPECTED_SUITE_NAMES=(
  collab_product
  collab_projection
  collab_lifecycle
  collab_shutdown
  document_collab_lifecycle
  revision_integration
  document_api_integration
  document_import_export_integration
  document_import_formats_integration
  task_collab_integration
)

mapfile -t EXPECTED_TEST_TARGETS < <(
  grep -E '^\s+--test ' "$RUN" | sed -E 's/^[[:space:]]+--test[[:space:]]+//; s/[[:space:]]*\\$//'
)

assert_expected_test_targets() {
  local args="$1"
  local target
  if ((${#EXPECTED_TEST_TARGETS[@]} != ${#EXPECTED_SUITE_NAMES[@]})); then
    echo "expected ${#EXPECTED_SUITE_NAMES[@]} --test targets in runner, found ${#EXPECTED_TEST_TARGETS[@]}" >&2
    return 1
  fi
  for target in "${EXPECTED_SUITE_NAMES[@]}"; do
    if [[ "$args" != *"--test ${target}"* ]]; then
      echo "cargo invocation missing --test ${target}" >&2
      echo "invocation: $args" >&2
      return 1
    fi
  done
  for target in "${EXPECTED_TEST_TARGETS[@]}"; do
    local found=0
    for expected in "${EXPECTED_SUITE_NAMES[@]}"; do
      if [[ "$target" == "$expected" ]]; then
        found=1
        break
      fi
    done
    if [[ "$found" -ne 1 ]]; then
      echo "unexpected --test target in runner: ${target}" >&2
      return 1
    fi
  done
}

# Each target exactly once across all invocations; only task_collab_integration
# runs single-threaded, alone in its own invocation.
assert_invocation_shape() {
  local all target count
  all="$(<"$INVOCATIONS")"
  for target in "${EXPECTED_SUITE_NAMES[@]}"; do
    count="$(grep -oE -- "--test ${target}( |$)" <<<"$all" | wc -l | tr -d ' ')"
    if [[ "$count" -ne 1 ]]; then
      echo "expected --test ${target} exactly once across invocations, found ${count}" >&2
      return 1
    fi
  done
  local threaded
  threaded="$(grep -c -- '-- --test-threads=1' "$INVOCATIONS" || true)"
  if [[ "$threaded" -ne 1 ]] || ! grep -- '-- --test-threads=1' "$INVOCATIONS" | grep -qE -- '--test task_collab_integration( |$)'; then
    echo "expected exactly one single-threaded invocation, for task_collab_integration" >&2
    return 1
  fi
  if [[ "$(grep -cE -- '--test ' "$INVOCATIONS")" -ne 2 ]] \
    || grep -- '--test task_collab_integration' "$INVOCATIONS" | grep -qE -- '--test (collab_|document_|revision_)'; then
    echo "task_collab_integration must run alone in its own invocation" >&2
    return 1
  fi
  if grep -qv 'MEILI=http://127.0.0.1:43210 ' "$INVOCATIONS"; then
    echo "every cargo invocation must see the wrapper's FVOCI_MEILI_URL" >&2
    return 1
  fi
}

cat >"$FAKE_BIN/cargo" <<STUB
#!/usr/bin/env bash
echo "MEILI=\${FVOCI_MEILI_URL:-unset} \$*" >>"$INVOCATIONS"
mode="\${FVOCI_TEST_CARGO_MODE:-pass}"
# The admission test lives in collab_product: only that invocation prints it.
admission=0
case " \$* " in *" --test collab_product "*) admission=1 ;; esac
second=0
case " \$* " in *" --test task_collab_integration "*) second=1 ;; esac
case "\$mode" in
  fail)
    [[ \$admission -eq 1 ]] && echo 'test collab_primary_huge_varint_memory_rejected_1008 ... ok'
    echo "error: simulated cargo test failure" >&2
    exit 101
    ;;
  fail-first)
    [[ \$admission -eq 1 ]] && echo 'test collab_primary_huge_varint_memory_rejected_1008 ... ok'
    if [[ \$second -eq 0 ]]; then
      echo "error: simulated cargo test failure" >&2
      exit 101
    fi
    exit 0
    ;;
  fail-second)
    [[ \$admission -eq 1 ]] && echo 'test collab_primary_huge_varint_memory_rejected_1008 ... ok'
    if [[ \$second -eq 1 ]]; then
      echo "error: simulated cargo test failure" >&2
      exit 102
    fi
    exit 0
    ;;
  fail-first-before-admission)
    if [[ \$second -eq 0 ]]; then
      echo "error: could not compile (simulated, before any test ran)" >&2
      exit 101
    fi
    exit 0
    ;;
  fail-second-no-admission)
    [[ \$admission -eq 1 ]] && echo 'test collab_delivery_admission_parity_with_locking_join ... ok'
    if [[ \$second -eq 1 ]]; then
      echo "error: simulated cargo test failure" >&2
      exit 102
    fi
    exit 0
    ;;
  fail-both)
    [[ \$admission -eq 1 ]] && echo 'test collab_primary_huge_varint_memory_rejected_1008 ... ok'
    echo "error: simulated cargo test failure" >&2
    [[ \$second -eq 1 ]] && exit 102
    exit 101
    ;;
  duplicate)
    if [[ \$admission -eq 1 ]]; then
      echo 'test collab_primary_huge_varint_memory_rejected_1008 ... ok'
      echo 'test collab_primary_huge_varint_memory_rejected_1008 ... ok'
    fi
    exit 0
    ;;
  missing)
    [[ \$admission -eq 1 ]] && echo 'test collab_delivery_admission_parity_with_locking_join ... ok'
    exit 0
    ;;
  failed)
    if [[ \$admission -eq 1 ]]; then
      echo 'test collab_primary_huge_varint_memory_rejected_1008 ... ok'
      echo 'test collab_primary_huge_varint_memory_rejected_1008 ... FAILED'
    fi
    exit 0
    ;;
  ignored)
    if [[ \$admission -eq 1 ]]; then
      echo 'test collab_primary_huge_varint_memory_rejected_1008 ... ok'
      echo 'test collab_primary_huge_varint_memory_rejected_1008 ... ignored'
    fi
    exit 0
    ;;
  pass)
    [[ \$admission -eq 1 ]] && echo 'test collab_primary_huge_varint_memory_rejected_1008 ... ok'
    echo 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s'
    exit 0
    ;;
  *)
    echo "unknown FVOCI_TEST_CARGO_MODE=\$mode" >&2
    exit 2
    ;;
esac
STUB
chmod +x "$FAKE_BIN/cargo"

# Fake docker/openssl for the real scripts/start-test-meili.sh: records calls,
# starts nothing.
cat >"$FAKE_BIN/docker" <<STUB
#!/usr/bin/env bash
echo "\$*" >>"$DOCKER_CALLS"
case "\$1" in
  run) echo fixturecid ;;
  port) echo 127.0.0.1:43210 ;;
  exec|rm|logs) : ;;
  *) echo "unexpected docker \$1" >&2; exit 2 ;;
esac
STUB
cat >"$FAKE_BIN/openssl" <<'STUB'
#!/usr/bin/env bash
[[ "$1 $2" == "rand -hex" ]] || { echo "unexpected openssl $*" >&2; exit 2; }
echo 00112233445566778899aabbccddeeff
STUB
chmod +x "$FAKE_BIN/docker" "$FAKE_BIN/openssl"

export PATH="$FAKE_BIN:$PATH"

run_runner() {
  : >"$INVOCATIONS"
  : >"$DOCKER_CALLS"
  FVOCI_TEST_CARGO_MODE="$1" bash "$RUN"
}

if ! run_runner pass >/dev/null; then
  echo "expected runner success with one admission ok line" >&2
  exit 1
fi
invocation_count="$(wc -l <"$INVOCATIONS" | tr -d ' ')"
if [[ "$invocation_count" -ne 2 ]]; then
  echo "expected exactly two cargo test invocations, got ${invocation_count}" >&2
  cat "$INVOCATIONS" >&2
  exit 1
fi
assert_expected_test_targets "$(<"$INVOCATIONS")"
assert_invocation_shape
if [[ "$(grep -c '^run ' "$DOCKER_CALLS")" -ne 1 ]] || [[ "$(grep -c '^rm ' "$DOCKER_CALLS")" -ne 1 ]]; then
  echo "expected one isolated Meilisearch for both invocations, started and removed once" >&2
  cat "$DOCKER_CALLS" >&2
  exit 1
fi

# The first non-zero cargo status is returned as is (101 from the first
# invocation, 102 from the second), never a substituted condition status.
# A cargo failure also wins over a failing admission check (no admission line
# because the first invocation never ran its tests, or the line is missing).
for case_ in fail:101 fail-first:101 fail-second:102 fail-both:101 \
  fail-first-before-admission:101 fail-second-no-admission:102; do
  mode="${case_%%:*}"
  expected="${case_##*:}"
  status=0
  run_runner "$mode" >/dev/null 2>&1 || status=$?
  if [[ "$status" -ne "$expected" ]]; then
    echo "expected runner to propagate cargo exit ${expected} in ${mode}, got ${status}" >&2
    exit 1
  fi
  if [[ "$(wc -l <"$INVOCATIONS" | tr -d ' ')" -ne 2 ]]; then
    echo "expected both cargo invocations to run in ${mode}" >&2
    exit 1
  fi
done

if run_runner duplicate >/dev/null 2>&1; then
  echo "expected runner failure when admission test passes twice" >&2
  exit 1
fi

# Both cargo invocations succeed but the admission line is missing: the
# admission check alone fails the runner, with its own status (1).
status=0
run_runner missing >/dev/null 2>&1 || status=$?
if [[ "$status" -ne 1 ]]; then
  echo "expected admission check failure (1) when admission test is absent, got ${status}" >&2
  exit 1
fi

if run_runner failed >/dev/null 2>&1; then
  echo "expected runner failure when admission test FAILED appears in log" >&2
  exit 1
fi

if run_runner ignored >/dev/null 2>&1; then
  echo "expected runner failure when admission test is ignored in log" >&2
  exit 1
fi

echo "run-collaboration-admission-fixture-test: ok"
