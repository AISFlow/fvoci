#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TSX="$ROOT/scripts/document-convert/node_modules/tsx/dist/loader.mjs"
CONVERT="$ROOT/scripts/document-convert/convert.mjs"
exec node --import "$TSX" "$CONVERT"
