#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
exec bun --no-env-file "$ROOT/tools/web-e2e/shard-fixture.ts" ci-shard "$ROOT"
