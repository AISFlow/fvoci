#!/usr/bin/env bash
# Test-only: collaboration CI runner with stubbed cargo.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
RUN="$ROOT/scripts/run-rust-collaboration-ci-tests.sh"
FIXTURE_RUN="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-rust-ci-admission-fixture.XXXXXX")"
FAKE_BIN="$FIXTURE_RUN/fake-bin"
INVOCATIONS="$FIXTURE_RUN/cargo-invocations.log"

cleanup() {
  rm -rf "$FIXTURE_RUN"
}
trap cleanup EXIT

mkdir -p "$FAKE_BIN"
export RUNNER_TEMP="$FIXTURE_RUN/logs"

mapfile -t EXPECTED_TEST_TARGETS < <(
  grep -E '^\s+--test ' "$RUN" | sed -E 's/^[[:space:]]+--test[[:space:]]+//; s/[[:space:]]*\\$//'
)

assert_expected_test_targets() {
  local args="$1"
  local target
  for target in "${EXPECTED_TEST_TARGETS[@]}"; do
    if [[ "$args" != *"--test ${target}"* ]]; then
      echo "cargo invocation missing --test ${target}" >&2
      echo "invocation: $args" >&2
      return 1
    fi
  done
  if ((${#EXPECTED_TEST_TARGETS[@]} != 9)); then
    echo "expected nine --test targets in runner, found ${#EXPECTED_TEST_TARGETS[@]}" >&2
    return 1
  fi
}

cat >"$FAKE_BIN/cargo" <<STUB
#!/usr/bin/env bash
echo "\$*" >>"$INVOCATIONS"
mode="\${FVOCI_TEST_CARGO_MODE:-pass}"
case "\$mode" in
  fail)
    cat <<'OUT'
test collab_primary_huge_varint_memory_rejected_1008 ... ok
OUT
    echo "error: simulated cargo test failure" >&2
    exit 101
    ;;
  duplicate)
    cat <<'OUT'
test collab_primary_huge_varint_memory_rejected_1008 ... ok
test collab_primary_huge_varint_memory_rejected_1008 ... ok
OUT
    exit 0
    ;;
  missing)
    echo 'test collab_delivery_admission_parity_with_locking_join ... ok'
    exit 0
    ;;
  failed)
    cat <<'OUT'
test collab_primary_huge_varint_memory_rejected_1008 ... ok
test collab_primary_huge_varint_memory_rejected_1008 ... FAILED
OUT
    exit 0
    ;;
  ignored)
    cat <<'OUT'
test collab_primary_huge_varint_memory_rejected_1008 ... ok
test collab_primary_huge_varint_memory_rejected_1008 ... ignored
OUT
    exit 0
    ;;
  pass)
    cat <<'OUT'
test collab_primary_huge_varint_memory_rejected_1008 ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
OUT
    exit 0
    ;;
  *)
    echo "unknown FVOCI_TEST_CARGO_MODE=\$mode" >&2
    exit 2
    ;;
esac
STUB
chmod +x "$FAKE_BIN/cargo"

export PATH="$FAKE_BIN:$PATH"

run_runner() {
  FVOCI_TEST_CARGO_MODE="$1" bash "$RUN"
}

: >"$INVOCATIONS"
if ! run_runner pass >/dev/null; then
  echo "expected runner success with one admission ok line" >&2
  exit 1
fi
invocation_count="$(wc -l <"$INVOCATIONS" | tr -d ' ')"
if [[ "$invocation_count" -ne 1 ]]; then
  echo "expected exactly one cargo test invocation, got ${invocation_count}" >&2
  cat "$INVOCATIONS" >&2
  exit 1
fi
assert_expected_test_targets "$(<"$INVOCATIONS")"

fail_status=0
if run_runner fail >/dev/null 2>&1; then
  fail_status=0
else
  fail_status=$?
fi
if [[ "$fail_status" -eq 0 ]]; then
  echo "expected runner failure when cargo exits non-zero" >&2
  exit 1
fi
if [[ "$fail_status" -ne 101 ]]; then
  echo "expected runner to propagate cargo exit 101, got ${fail_status}" >&2
  exit 1
fi

if run_runner duplicate >/dev/null 2>&1; then
  echo "expected runner failure when admission test passes twice" >&2
  exit 1
fi

if run_runner missing >/dev/null 2>&1; then
  echo "expected runner failure when admission test is absent" >&2
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
