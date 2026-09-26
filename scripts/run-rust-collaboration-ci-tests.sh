#!/usr/bin/env bash
# CI collaboration job: one offline cargo test invocation with captured output.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG_DIR="${RUNNER_TEMP:-/tmp}"
mkdir -p "$LOG_DIR"
LOG="$LOG_DIR/native-admission-test.log"
ADMISSION_TEST='collab_primary_huge_varint_memory_rejected_1008'

verify_native_admission_log() {
  local log="$1"
  local ok_pattern="^test ${ADMISSION_TEST} \\.\\.\\. ok\$"

  if grep -qE "^test ${ADMISSION_TEST} \\.\\.\\. FAILED" "$log"; then
    echo "native admission test failed in captured output" >&2
    return 1
  fi
  if grep -qE "^test ${ADMISSION_TEST} \\.\\.\\. ignored" "$log"; then
    echo "native admission test was ignored in captured output" >&2
    return 1
  fi

  local count
  count="$(grep -cE "$ok_pattern" "$log" || true)"
  if [[ "$count" -ne 1 ]]; then
    echo "expected exactly one passing run of ${ADMISSION_TEST}, found ${count}" >&2
    return 1
  fi
}

cd "$ROOT"
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
  | tee "$LOG"

verify_native_admission_log "$LOG"
