#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO="$(cd "$ROOT" && rustup which cargo)"
exec "$CARGO" run --quiet --locked --manifest-path "$ROOT/xtask/Cargo.toml" -- start-test postgres "$@"
