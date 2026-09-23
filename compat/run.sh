#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
if [[ -n "${CARGO_HOME:-}" ]]; then
  export PATH="$CARGO_HOME/bin:$PATH"
fi
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export YRS_BRIDGE="${YRS_BRIDGE:-$CARGO_TARGET_DIR/debug/yrs-bridge}"

cd "$ROOT"
cargo build --locked --offline --bins
timeout 30s node js/probe.mjs
timeout 30s node js/hocuspocus-handshake.mjs
"$CARGO_TARGET_DIR/debug/extract-probe" \
  fixtures/sample.pdf \
  fixtures/sample.docx \
  fixtures/sample.hwpx \
  fixtures/sample.hwp
