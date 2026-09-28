#!/usr/bin/env bash
# A release commit must already be green: the latest run of every CI gate
# (scripts/ci_selection.py WORKFLOW_JOBS -> <workflow>-ci-gate) on that exact
# SHA concluded success. Needs GH_TOKEN with checks: read and GITHUB_REPOSITORY.
#
#   scripts/release-check-ci.sh <40-hex sha>
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SHA="${1:?usage: release-check-ci.sh <sha>}"
[[ "$SHA" =~ ^[0-9a-f]{40}$ ]] || { echo "not a full commit SHA: $SHA" >&2; exit 2; }
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"

GATES="$(cd "$ROOT" && python3 -c 'import sys; sys.path.insert(0, "scripts"); import ci_selection as s; print(" ".join(s.gate_job_id(w) for w in s.WORKFLOW_JOBS))')"
RUNS="$(gh api --paginate "repos/${GITHUB_REPOSITORY}/commits/${SHA}/check-runs?per_page=100" \
  --jq '.check_runs[] | [.name, .status, (.conclusion // "none"), (.completed_at // .started_at // "")] | @tsv')"

python3 - "$GATES" "$RUNS" <<'PY'
import sys
gates = sys.argv[1].split()
latest = {}
for row in sys.argv[2].splitlines():
    name, status, conclusion, at = row.split("\t")
    if name in gates and (name not in latest or at > latest[name][0]):
        latest[name] = (at, status, conclusion)
bad = []
for gate in gates:
    at, status, conclusion = latest.get(gate, ("", "missing", "none"))
    print(f"{gate}: {status} {conclusion} {at}")
    if status != "completed" or conclusion != "success":
        bad.append(gate)
if bad:
    sys.exit(f"release commit is not green on: {', '.join(bad)}")
PY
