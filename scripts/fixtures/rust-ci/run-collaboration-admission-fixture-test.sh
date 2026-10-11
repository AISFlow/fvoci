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
RUNNER_STDERR="$FIXTURE_RUN/runner-stderr.log"
READY_FIFO="$FIXTURE_RUN/cargo-ready.fifo"
HOLD_FIFO="$FIXTURE_RUN/cargo-hold.fifo"
STDERR_FIFO="$FIXTURE_RUN/runner-stderr.fifo"
signal_pgid=""

cleanup() {
  if [[ -n "$signal_pgid" ]]; then
    kill -KILL -- "-$signal_pgid" 2>/dev/null || true
  fi
  rm -rf "$FIXTURE_RUN"
}
trap cleanup EXIT

mkdir -p "$FAKE_BIN"
mkfifo "$READY_FIFO" "$HOLD_FIFO" "$STDERR_FIFO"

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
# cargo --no-fail-fast's stderr summary shape, naming this invocation's first target.
args=" \$* "
fail_summary() {
  local target="\${args#* --test }"
  echo "error: 1 target failed:" >&2
  echo "    \\\`--test \${target%% *}\\\`" >&2
}
case "\$mode" in
  hang)
    # Signal readiness, then block (no writer ever opens the hold fifo) until
    # the fixture's SIGINT to the process group ends this process.
    echo "\$\$" >"$READY_FIFO"
    read -r _ <"$HOLD_FIFO"
    exit 0
    ;;
  fail)
    [[ \$admission -eq 1 ]] && echo 'test collab_primary_huge_varint_memory_rejected_1008 ... ok'
    fail_summary
    exit 101
    ;;
  fail-first)
    [[ \$admission -eq 1 ]] && echo 'test collab_primary_huge_varint_memory_rejected_1008 ... ok'
    if [[ \$second -eq 0 ]]; then
      fail_summary
      exit 101
    fi
    exit 0
    ;;
  fail-second)
    [[ \$admission -eq 1 ]] && echo 'test collab_primary_huge_varint_memory_rejected_1008 ... ok'
    if [[ \$second -eq 1 ]]; then
      fail_summary
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
      fail_summary
      exit 102
    fi
    exit 0
    ;;
  fail-both)
    [[ \$admission -eq 1 ]] && echo 'test collab_primary_huge_varint_memory_rejected_1008 ... ok'
    fail_summary
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
[[ -z "${FVOCI_TEST_OPENSSL_FAIL:-}" ]] || { echo "simulated openssl failure" >&2; exit 1; }
echo 00112233445566778899aabbccddeeff
STUB
chmod +x "$FAKE_BIN/docker" "$FAKE_BIN/openssl"

export PATH="$FAKE_BIN:$PATH"

# Each run gets its own RUNNER_TEMP, so no log carries over between modes.
fresh_run() {
  : >"$INVOCATIONS"
  : >"$DOCKER_CALLS"
  RUNNER_TEMP="$(mktemp -d "$FIXTURE_RUN/runner-temp.XXXXXX")"
  export RUNNER_TEMP
}

run_runner() {
  fresh_run
  FVOCI_TEST_CARGO_MODE="$1" bash "$RUN" 2>"$RUNNER_STDERR"
}

assert_meili_started_and_removed_once() {
  if [[ "$(grep -c '^run ' "$DOCKER_CALLS")" -ne 1 ]] || [[ "$(grep -c '^rm ' "$DOCKER_CALLS")" -ne 1 ]]; then
    echo "expected one isolated Meilisearch, started and removed once ($1)" >&2
    cat "$DOCKER_CALLS" >&2
    exit 1
  fi
}

if ! run_runner pass >/dev/null; then
  echo "expected runner success with one admission ok line" >&2
  cat "$RUNNER_STDERR" >&2
  exit 1
fi
for log in parallel-suites.stdout.log parallel-suites.stderr.log \
  task_collab_integration.stdout.log task_collab_integration.stderr.log; do
  if [[ ! -f "$RUNNER_TEMP/rust-collaboration-logs/$log" ]]; then
    echo "expected collaboration log $log under RUNNER_TEMP/rust-collaboration-logs" >&2
    exit 1
  fi
done
invocation_count="$(wc -l <"$INVOCATIONS" | tr -d ' ')"
if [[ "$invocation_count" -ne 2 ]]; then
  echo "expected exactly two cargo test invocations, got ${invocation_count}" >&2
  cat "$INVOCATIONS" >&2
  exit 1
fi
assert_expected_test_targets "$(<"$INVOCATIONS")"
assert_invocation_shape
assert_meili_started_and_removed_once "both invocations"

# A failed invocation is named on stderr with its log, and cargo's error line is
# repeated after cargo's own live copy (two copies); a passing one is not named.
assert_failure_named() {
  local mode="$1" label="$2" line="$3" log="$4" copies
  copies="$(grep -cxF -- "$line" "$RUNNER_STDERR" || true)"
  if ! grep -qF "collaboration: ${label} exited " "$RUNNER_STDERR" \
    || [[ "$copies" -ne 2 ]] \
    || ! grep -qxF -- "$line" "$RUNNER_TEMP/rust-collaboration-logs/${log}.stderr.log"; then
    echo "expected ${mode} to name failing invocation '${label}' with: ${line}" >&2
    cat "$RUNNER_STDERR" >&2
    exit 1
  fi
}
SERIAL_LABEL='task_collab_integration (--test-threads=1)'
assert_failure_not_named() {
  local mode="$1" label="$2"
  if grep -qF "collaboration: ${label} exited " "$RUNNER_STDERR"; then
    echo "${mode}: passing invocation '${label}' must not be reported as failed" >&2
    exit 1
  fi
}

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
  assert_meili_started_and_removed_once "$mode"
  # shellcheck disable=SC2016 # literal backticks in cargo's summary
  parallel_line='    `--test collab_product`'
  # shellcheck disable=SC2016
  serial_line='    `--test task_collab_integration`'
  case "$mode" in
    fail | fail-both)
      assert_failure_named "$mode" "parallel suites" "$parallel_line" parallel-suites
      assert_failure_named "$mode" "$SERIAL_LABEL" "$serial_line" task_collab_integration
      ;;
    fail-first)
      assert_failure_named "$mode" "parallel suites" "$parallel_line" parallel-suites
      assert_failure_not_named "$mode" "$SERIAL_LABEL"
      ;;
    fail-first-before-admission)
      assert_failure_named "$mode" "parallel suites" \
        'error: could not compile (simulated, before any test ran)' parallel-suites
      assert_failure_not_named "$mode" "$SERIAL_LABEL"
      ;;
    fail-second | fail-second-no-admission)
      assert_failure_not_named "$mode" "parallel suites"
      assert_failure_named "$mode" "$SERIAL_LABEL" "$serial_line" task_collab_integration
      ;;
  esac
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

# A start that fails before creating a container removes nothing, not even a
# container name inherited from a parent wrapper's environment.
fresh_run
status=0
FVOCI_TEST_MEILI_CONTAINER=parent-meili FVOCI_TEST_OPENSSL_FAIL=1 FVOCI_TEST_CARGO_MODE=pass \
  bash "$RUN" >/dev/null 2>"$RUNNER_STDERR" || status=$?
if [[ "$status" -eq 0 ]] || [[ -s "$INVOCATIONS" ]] || grep -q '^rm ' "$DOCKER_CALLS"; then
  echo "expected failed Meilisearch start to fail the runner, run no cargo and remove nothing (status ${status})" >&2
  cat "$DOCKER_CALLS" >&2
  exit 1
fi

# INT or TERM to the runner's process group while cargo runs (Ctrl-C or a
# runner cancel): the runner exits 130/143 itself (never killed by SIGPIPE
# or the signal), never starts the second invocation, and its EXIT trap removes
# the Meilisearch once. set -m gives the background runner its own group with
# INT not ignored. Runner stderr goes through a fifo whose reader ends only
# when every writer, including the runner's stderr tee, has exited.
signal_case() {
  local signal="$1" expected="$2" cat_pid cargo_pid status reader=0
  fresh_run
  timeout 60 cat <"$STDERR_FIFO" >"$RUNNER_STDERR" &
  cat_pid=$!
  exec 3<>"$READY_FIFO"
  set -m
  FVOCI_TEST_CARGO_MODE=hang bash "$RUN" >/dev/null 2>"$STDERR_FIFO" 3<&- &
  signal_pgid=$!
  set +m
  cargo_pid=""
  if ! read -r -t 60 cargo_pid <&3; then
    echo "hang-mode cargo stub never started (${signal})" >&2
    exit 1
  fi
  exec 3<&-
  kill "-${signal}" -- "-$signal_pgid"
  status=0
  wait "$signal_pgid" || status=$?
  wait "$cat_pid" || reader=$?
  if [[ "$status" -ne "$expected" ]]; then
    echo "expected runner exit ${expected} after SIG${signal}, got ${status}" >&2
    cat "$RUNNER_STDERR" >&2
    exit 1
  fi
  if [[ "$reader" -ne 0 ]]; then
    echo "runner stderr stayed open after SIG${signal} (a tee outlived the runner): ${reader}" >&2
    exit 1
  fi
  if kill -0 "$cargo_pid" 2>/dev/null; then
    echo "cargo stub ${cargo_pid} still running after the runner exited (${signal})" >&2
    exit 1
  fi
  signal_pgid=""
  if [[ "$(wc -l <"$INVOCATIONS" | tr -d ' ')" -ne 1 ]]; then
    echo "expected SIG${signal} to stop the runner before the second cargo invocation" >&2
    cat "$INVOCATIONS" >&2
    exit 1
  fi
  assert_meili_started_and_removed_once "SIG${signal}"
}

signal_case INT 130
signal_case TERM 143

echo "run-collaboration-admission-fixture-test: ok"
