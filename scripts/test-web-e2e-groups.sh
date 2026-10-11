#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

shard_count="$(bun tools/web-e2e/groups.ts shards)"
bun tools/web-e2e/groups.ts verify --shards "$shard_count"
bun --bun node_modules/typescript/bin/tsc -p tools/web-e2e/tsconfig.json
bun test --timeout=60000 ./tools/web-e2e/groups.test.ts ./tools/web-e2e/trace-summary.test.ts \
  ./tools/web-e2e/smtp-sink.test.ts ./tools/web-e2e/run-web-e2e.test.ts \
  ./tools/web-e2e/network-settle.test.ts ./tools/web-e2e/database-urls.test.ts

bash scripts/fixtures/web-e2e/run-ci-shard-fixture-test.sh
bash scripts/fixtures/web-e2e/failure-output-fixture-test.sh
bash scripts/fixtures/web-e2e/trace-summary-fixture-test.sh

bash -n scripts/run-web-e2e.sh
bash -n scripts/web-e2e-run-group.sh
bash -n scripts/web-e2e-inner.sh

echo "test-web-e2e-groups: ok"
