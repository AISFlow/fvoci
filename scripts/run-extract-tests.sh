#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target/extract-job}"

if [[ -z "${TEST_DATABASE_URL:-}" ]]; then
  echo "TEST_DATABASE_URL is required" >&2
  exit 1
fi

cd "$ROOT"

echo "==> cargo check (extract-job target)"
cargo check --target-dir "$TARGET"

echo "==> DB policy tests"
cargo test --locked --features db-tests --test attachment_extract_integration --target-dir "$TARGET" -- --nocapture

echo "==> build native helper"
cargo build -p document-extract --target-dir "$TARGET" --locked
export FVOCI_EXTRACTOR_BIN="$TARGET/debug/document-extract"
if [[ ! -x "$FVOCI_EXTRACTOR_BIN" ]]; then
  echo "missing helper binary at $FVOCI_EXTRACTOR_BIN" >&2
  exit 1
fi

echo "==> native extract product tests"
cargo test --locked --features extract-native-tests --test attachment_extract_native --target-dir "$TARGET" -- --nocapture
