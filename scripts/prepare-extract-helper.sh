#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target/extract-job}"

cd "$ROOT/crates/document-extract"
if [[ ! -d .vendor-src/rhwp ]]; then
  bash fetch-rhwp.sh
fi
# Feature changes do not always invalidate the cached helper artifact; remove it so
# test-hang is definitely compiled into the subprocess used by lifecycle tests.
rm -f "$TARGET/debug/document-extract"
cargo build --locked --bin document-extract --features test-hang --target-dir "$TARGET"

export FVOCI_EXTRACTOR_BIN="$TARGET/debug/document-extract"
if [[ ! -x "$FVOCI_EXTRACTOR_BIN" ]]; then
  echo "missing helper binary at $FVOCI_EXTRACTOR_BIN" >&2
  exit 1
fi
echo "FVOCI_EXTRACTOR_BIN=$FVOCI_EXTRACTOR_BIN"
