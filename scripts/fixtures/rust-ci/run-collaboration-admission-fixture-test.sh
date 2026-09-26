#!/usr/bin/env bash
# Test-only: native admission log proof and pipefail cargo failure propagation.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
VERIFY="$ROOT/scripts/verify-collaboration-native-admission-log.sh"
RUN="$ROOT/scripts/run-rust-collaboration-ci-tests.sh"
FAKE_BIN="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-rust-ci-fake-bin.XXXXXX")"

cleanup() {
  rm -rf "$FAKE_BIN"
}
trap cleanup EXIT

chmod +x "$VERIFY" "$RUN"

assert_verify_fails() {
  local log="$1"
  if bash "$VERIFY" "$log"; then
    echo "expected verify failure for log: $log" >&2
    exit 1
  fi
}

assert_verify_ok() {
  local log="$1"
  bash "$VERIFY" "$log"
}

log_ok="$(mktemp)"
cat >"$log_ok" <<'EOF'
test collab_primary_huge_varint_memory_rejected_1008 ... ok
test collab_delivery_admission_parity_with_locking_join ... ok
EOF
assert_verify_ok "$log_ok"

log_zero="$(mktemp)"
echo 'test collab_delivery_admission_parity_with_locking_join ... ok' >"$log_zero"
assert_verify_fails "$log_zero"

log_twice="$(mktemp)"
cat >"$log_twice" <<'EOF'
test collab_primary_huge_varint_memory_rejected_1008 ... ok
test collab_primary_huge_varint_memory_rejected_1008 ... ok
EOF
assert_verify_fails "$log_twice"

log_failed="$(mktemp)"
cat >"$log_failed" <<'EOF'
test collab_primary_huge_varint_memory_rejected_1008 ... FAILED
EOF
assert_verify_fails "$log_failed"

log_ignored="$(mktemp)"
echo 'test collab_primary_huge_varint_memory_rejected_1008 ... ignored' >"$log_ignored"
assert_verify_fails "$log_ignored"

cat >"$FAKE_BIN/cargo" <<'STUB'
#!/usr/bin/env bash
if [[ "${FVOCI_TEST_CARGO_MODE:-pass}" == "fail" ]]; then
  echo "error: simulated cargo test failure" >&2
  exit 101
fi
cat <<'OUT'
test collab_primary_huge_varint_memory_rejected_1008 ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
OUT
exit 0
STUB
chmod +x "$FAKE_BIN/cargo"

export PATH="$FAKE_BIN:$PATH"
export RUNNER_TEMP="${TMPDIR:-/tmp}/fvoci-rust-ci-admission-fixture"

log_run="$(mktemp)"
if ! bash "$RUN" >"$log_run" 2>&1; then
  echo "expected collaboration CI runner to succeed with stub cargo" >&2
  cat "$log_run" >&2
  exit 1
fi
if ! grep -q 'test collab_primary_huge_varint_memory_rejected_1008 ... ok' "$log_run"; then
  echo "stub cargo output missing from runner log" >&2
  cat "$log_run" >&2
  exit 1
fi

export FVOCI_TEST_CARGO_MODE=fail
if bash "$RUN" >/dev/null 2>&1; then
  echo "expected collaboration CI runner to fail when cargo exits non-zero" >&2
  exit 1
fi
unset FVOCI_TEST_CARGO_MODE

# Baseline CI log ran the admission test twice; verifier must reject that shape.
baseline_snippet="$(mktemp)"
if [[ -f /tmp/fvoci-ci-collab-duplicate-baseline.log ]]; then
  rg 'test collab_primary_huge_varint_memory_rejected_1008' \
    /tmp/fvoci-ci-collab-duplicate-baseline.log \
    | sed -E 's/.*(test collab_primary_huge_varint_memory_rejected_1008.*)/\1/' \
    >"$baseline_snippet" || true
fi
if [[ -s "$baseline_snippet" ]]; then
  assert_verify_fails "$baseline_snippet"
fi

echo "run-collaboration-admission-fixture-test: ok"
