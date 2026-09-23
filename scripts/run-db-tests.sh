#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"

if [[ -z "${TEST_DATABASE_URL:-}" ]]; then
  if [[ -f /tmp/fvoci-rust-b01d-jpu9ngdq/admin-url ]]; then
    export TEST_DATABASE_URL="$(cat /tmp/fvoci-rust-b01d-jpu9ngdq/admin-url)"
  else
    echo "TEST_DATABASE_URL is required for db-tests" >&2
    exit 1
  fi
fi

cd "$ROOT"
cargo test --features db-tests --test db_integration -- --nocapture
