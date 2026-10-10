#!/usr/bin/env bash
# Prints the measurement environment as JSON (no secrets, no user data).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SERVER_BIN="${1:?server binary}"
COLLAB_BIN="${2:?collab engine binary}"
exec bun "$ROOT/tools/perf/capture-env.ts" "$ROOT" "$SERVER_BIN" "$COLLAB_BIN"
