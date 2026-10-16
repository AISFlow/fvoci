#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec bun --no-env-file "$ROOT/tools/web-e2e/run-web-e2e.ts" entry "$ROOT" "$@"
