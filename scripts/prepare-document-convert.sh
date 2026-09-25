#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT/packages/editor"
npm install --no-fund --no-audit
cd "$ROOT/scripts/document-convert"
npm install --no-fund --no-audit
CONVERT="$ROOT/scripts/document-convert/convert.mjs"
RUNNER="$ROOT/scripts/run-document-convert.sh"
if [[ ! -f "$CONVERT" ]]; then
  echo "missing document convert script at $CONVERT" >&2
  exit 1
fi
if [[ ! -x "$RUNNER" ]]; then
  chmod +x "$RUNNER"
fi
export FVOCI_DOCUMENT_CONVERT_BIN="$RUNNER"
echo "FVOCI_DOCUMENT_CONVERT_BIN=$FVOCI_DOCUMENT_CONVERT_BIN"
