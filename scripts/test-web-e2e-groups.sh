#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

python3 scripts/web-e2e-groups.py verify --shards 8
python3 scripts/web-e2e-groups.py compare-legacy

# Pair must stay adjacent in shard output when co-located (same line).
pair_line="$(python3 scripts/web-e2e-groups.py list-groups | grep -F 'workspace-flow.spec.ts' || true)"
if [[ "$pair_line" != *"workspace-wiki-flow.spec.ts"* ]]; then
  echo "workspace flow pair not grouped on one line: $pair_line" >&2
  exit 1
fi

# Every shard must receive at least one group.
for shard in $(seq 0 7); do
  count="$(python3 scripts/web-e2e-groups.py shard --index "$shard" --shards 8 | wc -l)"
  if [[ "$count" -lt 1 ]]; then
    echo "shard $shard is empty" >&2
    exit 1
  fi
done

# run-web-e2e-shard must not call the full wrapper per group (build once).
if grep -q 'run-web-e2e\.sh' scripts/run-web-e2e-shard.sh; then
  echo "run-web-e2e-shard.sh must not re-invoke run-web-e2e.sh per group" >&2
  exit 1
fi
if ! grep -q 'build_current_artifacts' scripts/run-web-e2e-shard.sh; then
  echo "run-web-e2e-shard.sh must build once" >&2
  exit 1
fi

# collaboration-flow still uses run-web-e2e.sh without spec args under FVOCI_E2E_PENDING.
if ! grep -q 'FVOCI_E2E_PENDING' scripts/web-e2e-run-group.sh; then
  echo "web-e2e-run-group.sh must allow pending collab with no spec args" >&2
  exit 1
fi

echo "test-web-e2e-groups: ok"
