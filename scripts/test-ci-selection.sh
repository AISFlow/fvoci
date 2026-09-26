#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
python3 scripts/ci_selection.py verify-workflows
python3 -m unittest scripts.test_ci_selection -v
