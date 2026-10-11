#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
bun tools/ci/verify-workflows.ts
bun test ./tools/ci/planner/ ./tools/ci/gate/ ./tools/ci/verify/ \
  ./tools/ci/argv.test.ts ./tools/ci/workflows.test.ts
