#!/usr/bin/env bash
# Test-only: collaboration CI runner with stubbed cargo.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
RUN="$ROOT/scripts/run-rust-collaboration-ci-tests.sh"
FAKE_BIN="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-rust-ci-fake-bin.XXXXXX")"

cleanup() {
  rm -rf "$FAKE_BIN"
}
trap cleanup EXIT

chmod +x "$RUN"

cat >"$FAKE_BIN/cargo" <<'STUB'
#!/usr/bin/env bash
mode="${FVOCI_TEST_CARGO_MODE:-pass}"
case "$mode" in
  fail)
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
  pass)
    cat <<'OUT'
test collab_primary_huge_varint_memory_rejected_1008 ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
OUT
    exit 0
    ;;
  *)
    echo "unknown FVOCI_TEST_CARGO_MODE=$mode" >&2
    exit 2
    ;;
esac
STUB
chmod +x "$FAKE_BIN/cargo"

export PATH="$FAKE_BIN:$PATH"
export RUNNER_TEMP="${TMPDIR:-/tmp}/fvoci-rust-ci-admission-fixture"

run_runner() {
  FVOCI_TEST_CARGO_MODE="$1" bash "$RUN"
}

if ! run_runner pass >/dev/null; then
  echo "expected runner success with one admission ok line" >&2
  exit 1
fi

if run_runner fail >/dev/null 2>&1; then
  echo "expected runner failure when cargo exits non-zero" >&2
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

echo "run-collaboration-admission-fixture-test: ok"
