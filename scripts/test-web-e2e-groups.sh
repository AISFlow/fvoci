#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

python3 scripts/web-e2e-groups.py verify --shards 8
python3 scripts/test_web_e2e_groups.py -v

bash scripts/fixtures/web-e2e/run-ci-shard-fixture-test.sh

bash -n scripts/run-web-e2e.sh
bash -n scripts/web-e2e-run-group.sh

echo "test-web-e2e-groups: ok"
