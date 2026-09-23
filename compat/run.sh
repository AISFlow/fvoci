#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
export CARGO_HOME="${CARGO_HOME:-/home/kinesis/orca/toolchains/fvoci-rust/cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-/home/kinesis/orca/toolchains/fvoci-rust/rustup}"
export PATH="$CARGO_HOME/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export YRS_BRIDGE="${YRS_BRIDGE:-$CARGO_TARGET_DIR/debug/yrs-bridge}"

cd "$ROOT"
cargo build --bins
node js/probe.mjs
node js/hocuspocus-handshake.mjs
"$CARGO_TARGET_DIR/debug/extract-probe" \
  fixtures/sample.pdf \
  fixtures/sample.docx \
  fixtures/sample.hwpx \
  fixtures/sample.hwp
