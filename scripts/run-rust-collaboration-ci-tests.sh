#!/usr/bin/env bash
# CI collaboration job: one offline cargo test invocation with captured output.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG_DIR="${RUNNER_TEMP:-/tmp}"
mkdir -p "$LOG_DIR"
LOG="$LOG_DIR/native-admission-test.log"

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

bash "$ROOT/scripts/verify-collaboration-native-admission-log.sh" "$LOG"
