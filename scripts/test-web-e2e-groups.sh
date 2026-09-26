#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

python3 scripts/web-e2e-groups.py verify --shards 8
python3 scripts/test_web_e2e_groups.py -v

dry_log="$(mktemp)"
trap 'rm -f "$dry_log"; rm -rf "${tmpdir:-}"' EXIT

# Plan must succeed before a dry-run shard executes groups.
if ! FVOCI_WEB_E2E_DRY_RUN=1 bash scripts/run-web-e2e.sh --ci-shard 0 2>"$dry_log"; then
  cat "$dry_log" >&2
  exit 1
fi
build_once_count="$(grep -c 'fvoci-web-e2e-build-once' "$dry_log" || true)"
group_count="$(grep -c 'fvoci-web-e2e-run-group' "$dry_log" || true)"
if [[ "$build_once_count" -ne 1 ]]; then
  echo "expected exactly one build in --ci-shard dry run, got ${build_once_count}" >&2
  exit 1
fi
if [[ "$group_count" -lt 1 ]]; then
  echo "expected at least one group run in shard 0 dry run" >&2
  exit 1
fi

# Invalid shard plan must fail before build (dry run should not print build marker).
tmpdir="$(mktemp -d)"
export FVOCI_WEB_E2E_DIR="$tmpdir"
if FVOCI_WEB_E2E_DRY_RUN=1 bash scripts/run-web-e2e.sh --ci-shard 0 2>"$tmpdir/err.log"; then
  echo "expected failure for empty e2e fixture dir" >&2
  exit 1
fi
if grep -q 'fvoci-web-e2e-build-once' "$tmpdir/err.log"; then
  echo "build ran despite plan failure" >&2
  exit 1
fi

# Local default path still accepts pending collab with no spec args (syntax only).
bash -n scripts/run-web-e2e.sh
bash -n scripts/web-e2e-run-group.sh

echo "test-web-e2e-groups: ok"
