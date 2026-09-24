#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target/extract-job}"

# Resolve relative build output before changing directories.
TARGET="$(realpath -m "$TARGET")"
cd "$ROOT/crates/document-extract"
bash fetch-rhwp.sh
cargo fetch --locked
cargo build --locked --offline --bin document-extract --features test-hang --target-dir "$TARGET"
# Prepare the server dependencies explicitly; the test entrypoint is offline.
cd "$ROOT"
cargo fetch --locked

export FVOCI_EXTRACTOR_BIN="$TARGET/debug/document-extract"
if [[ ! -x "$FVOCI_EXTRACTOR_BIN" ]]; then
  echo "missing helper binary at $FVOCI_EXTRACTOR_BIN" >&2
  exit 1
fi
echo "FVOCI_EXTRACTOR_BIN=$FVOCI_EXTRACTOR_BIN"
