#!/usr/bin/env bash
# A release commit must already be green: the latest run of every CI gate
# (tools/ci/planner/registry.ts WORKFLOW_JOBS -> <workflow>-ci-gate) on that
# exact SHA concluded success (tools/release/check-ci.ts). Needs GH_TOKEN with
# checks: read and GITHUB_REPOSITORY.
#
#   scripts/release-check-ci.sh <40-hex sha>
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SHA="${1:?usage: release-check-ci.sh <sha>}"
[[ "$SHA" =~ ^[0-9a-f]{40}$ ]] || { echo "not a full commit SHA: $SHA" >&2; exit 2; }
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"

RUNS="$(gh api --paginate "repos/${GITHUB_REPOSITORY}/commits/${SHA}/check-runs?per_page=100" \
  --jq '.check_runs[] | [.name, .status, (.conclusion // "none"), (.completed_at // .started_at // "")] | @tsv')"

bun "$ROOT/tools/release/check-ci.ts" <<<"$RUNS"
