#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONVERT="$ROOT/scripts/document-convert/convert.mjs"
exec bun "$CONVERT"
