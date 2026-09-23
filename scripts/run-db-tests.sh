#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ -z "${TEST_DATABASE_URL:-}" ]]; then
  echo "TEST_DATABASE_URL is required for db-tests" >&2
  exit 1
fi

cd "$ROOT"
cargo test --locked --offline --features db-tests --test db_integration -- --nocapture
