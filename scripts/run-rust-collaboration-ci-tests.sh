#!/usr/bin/env bash
# CI collaboration job: two offline cargo test invocations with captured output,
# inside one isolated Meilisearch. task_collab_integration runs alone with
# --test-threads=1 (its personal-transfer cases each run a server and native
# seeds against the process-wide seed child cap); the other nine keep libtest's
# default parallelism. Both always run; the first non-zero status is returned.
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
# One isolated Meilisearch for both invocations: re-exec this script once
# inside the wrapper (it exports FVOCI_MEILI_URL/KEY and removes the container
# on exit); the Zotero real-search case requires it.
if [[ "${FVOCI_COLLAB_CI_IN_MEILI:-}" != 1 ]]; then
  FVOCI_COLLAB_CI_IN_MEILI=1 exec bash "$ROOT/scripts/start-test-meili.sh" bash "${BASH_SOURCE[0]}"
fi

status=0
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
  | tee "$LOG" || status=$?
cargo test --locked --offline --no-fail-fast --features db-tests \
  --test task_collab_integration \
  -- --test-threads=1 \
  | tee -a "$LOG" || { second=$?; [[ "$status" -ne 0 ]] || status=$second; }

verify_native_admission_log "$LOG"
exit "$status"
