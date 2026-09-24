#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target/extract-job}"

if [[ -z "${TEST_DATABASE_URL:-}" ]]; then
  echo "TEST_DATABASE_URL is required" >&2
  exit 1
fi

if [[ -z "${FVOCI_EXTRACTOR_BIN:-}" ]]; then
  echo "FVOCI_EXTRACTOR_BIN is required; run scripts/prepare-extract-helper.sh first" >&2
  exit 1
fi
if [[ ! -x "$FVOCI_EXTRACTOR_BIN" ]]; then
  echo "FVOCI_EXTRACTOR_BIN is not executable: $FVOCI_EXTRACTOR_BIN" >&2
  exit 1
fi

cd "$ROOT"

echo "==> cargo check (extract-job target, offline)"
cargo check --locked --offline --target-dir "$TARGET"

echo "==> clippy (extract-native-tests, all-targets, offline)"
cargo clippy --locked --offline --all-targets --features extract-native-tests --target-dir "$TARGET" -- -D warnings

echo "==> DB policy tests (offline)"
cargo test --locked --offline --features db-tests --test attachment_extract_integration --target-dir "$TARGET" -- --nocapture

echo "==> native extract lifecycle/product tests (offline)"
cargo test --locked --offline --features extract-native-tests --test attachment_extract_native --target-dir "$TARGET" -- --nocapture
