#!/usr/bin/env bash
# CI collaboration job: two offline cargo test invocations with captured output,
# inside one isolated Meilisearch. task_collab_integration runs alone with
# --test-threads=1 (its personal-transfer cases each run a server and native
# seeds against the process-wide seed child cap); the other nine keep libtest's
# default parallelism. Both always run; the first non-zero status is returned.
#
# This file is the only suite list: scripts/ci_selection.py reads the literal
# `cargo test` / `--test X \` lines below (keep that shape, one --test per line).
# Logs: $RUNNER_TEMP/rust-collaboration-logs in CI, else a fresh temp directory.
# stdout (libtest) and stderr (cargo status, child output) are kept apart so a
# child's stderr cannot split a `test X ... ok` line the admission check reads.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ -n "${RUNNER_TEMP:-}" ]]; then
  LOG_DIR="$RUNNER_TEMP/rust-collaboration-logs"
  mkdir -p "$LOG_DIR"
else
  LOG_DIR="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-rust-collaboration-logs.XXXXXX")"
fi
PARALLEL_LOG="$LOG_DIR/parallel-suites"
SERIAL_LOG="$LOG_DIR/task_collab_integration"
ADMISSION_TEST='collab_primary_huge_varint_memory_rejected_1008'

verify_native_admission_log() {
  local ok_pattern="^test ${ADMISSION_TEST} \\.\\.\\. ok\$"

  if grep -hqE "^test ${ADMISSION_TEST} \\.\\.\\. FAILED" "$@"; then
    echo "native admission test failed in captured output" >&2
    return 1
  fi
  if grep -hqE "^test ${ADMISSION_TEST} \\.\\.\\. ignored" "$@"; then
    echo "native admission test was ignored in captured output" >&2
    return 1
  fi

  local count
  count="$(cat "$@" | grep -cE "$ok_pattern" || true)"
  if [[ "$count" -ne 1 ]]; then
    echo "expected exactly one passing run of ${ADMISSION_TEST}, found ${count}" >&2
    return 1
  fi
}

# Names the failed invocation, its logs and cargo's own error lines
# (`--no-fail-fast` ends with `error: N targets failed:` and each `--test X`).
report_failure() {
  local label="$1" status="$2" log="$3"
  echo "collaboration: ${label} exited ${status}; logs: ${log}.stdout.log ${log}.stderr.log" >&2
  # shellcheck disable=SC2016 # literal backticks in cargo's summary
  grep -E '^error: ([0-9]+ targets? failed|test failed|could not compile)|^[[:space:]]+`--test [^`]+`$' \
    "${log}.stderr.log" >&2 || true
}

cd "$ROOT"
# One isolated Meilisearch for both invocations, owned by this process: the
# Zotero real-search case requires it.
# shellcheck source=scripts/start-test-meili.sh
source "$ROOT/scripts/start-test-meili.sh"
trap fvoci_test_meili_stop EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
fvoci_test_meili_start

# Each invocation: stdout teed to .stdout.log through the pipeline (pipefail
# keeps cargo's status), the group's stderr teed to .stderr.log by a process
# substitution this shell waits for before reading the log.
status=0
first=0
{
  cargo test --locked --offline --no-fail-fast --features db-tests \
    --test collab_product \
    --test collab_projection \
    --test collab_lifecycle \
    --test collab_shutdown \
    --test document_collab_lifecycle \
    --test revision_integration \
    --test document_api_integration \
    --test document_import_export_integration \
    --test document_import_formats_integration \
    | tee "$PARALLEL_LOG.stdout.log"
} 2> >(tee "$PARALLEL_LOG.stderr.log" >&2) || first=$?
wait "$!" || true
if [[ "$first" -ne 0 ]]; then
  report_failure "parallel suites" "$first" "$PARALLEL_LOG"
  status=$first
fi
second=0
{
  cargo test --locked --offline --no-fail-fast --features db-tests \
    --test task_collab_integration \
    -- --test-threads=1 \
    | tee "$SERIAL_LOG.stdout.log"
} 2> >(tee "$SERIAL_LOG.stderr.log" >&2) || second=$?
wait "$!" || true
if [[ "$second" -ne 0 ]]; then
  report_failure "task_collab_integration (--test-threads=1)" "$second" "$SERIAL_LOG"
  [[ "$status" -ne 0 ]] || status=$second
fi

# A cargo failure (e.g. a compile error or crash before the admission line
# printed) keeps its own status; the admission check decides only when both
# cargo invocations succeeded. It reads both stdout logs: one pass per run.
if [[ -z "${RUNNER_TEMP:-}" ]]; then
  echo "collaboration: logs in ${LOG_DIR}" >&2
fi
admission=0
verify_native_admission_log "$PARALLEL_LOG.stdout.log" "$SERIAL_LOG.stdout.log" || admission=$?
if [[ "$status" -ne 0 ]]; then
  exit "$status"
fi
if [[ "$admission" -ne 0 ]]; then
  echo "collaboration: native admission check failed; logs: ${LOG_DIR}" >&2
fi
exit "$admission"
