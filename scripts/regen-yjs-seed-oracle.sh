#!/usr/bin/env bash
# Regenerates tests/fixtures/yjs-seed/oracle/ from the document convert
# helper (the TS `tiptapJsonToYUpdate` oracle; dev/test only):
#   md-<name>.yupdate  base64 updateV1 of tests/fixtures/markdown-oracle/<name>.json
#   <case>.yupdate     base64 updateV1 of tests/fixtures/yjs-seed/cases/<case>.json
#   <case>.error       helper refusal code when the TS side throws
#   ../schema.json     editor schema attrs/defaults/mark overlap
# Requires FVOCI_DOCUMENT_CONVERT_BIN (see scripts/prepare-document-convert.sh).
# Client-side check of the Rust seed (Yjs + y-tiptap + editor schema), dev only:
#   bun scripts/document-convert/seed-client-check.mjs <collab-engine> <case.json>...
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/tests/fixtures/yjs-seed"
OUT="$DIR/oracle"
[[ -d "$DIR" ]] || { echo "missing yjs seed dir $DIR" >&2; exit 1; }
BIN="${FVOCI_DOCUMENT_CONVERT_BIN:?set FVOCI_DOCUMENT_CONVERT_BIN (scripts/prepare-document-convert.sh)}"

rm -rf "$OUT"
mkdir -p "$OUT"

seed() {
  local input="$1" name="$2" resp
  resp="$(jq -c '{op: "tiptap_to_yjs_update", contentJson: .}' "$input" | "$BIN")"
  if [[ "$(jq -r '.ok' <<<"$resp")" == "true" ]]; then
    jq -j '.updateB64' <<<"$resp" >"$OUT/$name.yupdate"
  else
    jq -j '.code' <<<"$resp" >"$OUT/$name.error"
  fi
}

shopt -s nullglob
md_json=("$ROOT"/tests/fixtures/markdown-oracle/*.json)
case_json=("$DIR"/cases/*.json)
if ((${#md_json[@]} == 0 || ${#case_json[@]} == 0)); then
  echo "missing yjs seed inputs (markdown-oracle json ${#md_json[@]}, cases ${#case_json[@]})" >&2
  exit 1
fi
for f in "${md_json[@]}"; do
  seed "$f" "md-$(basename "${f%.json}")"
done
for f in "${case_json[@]}"; do
  seed "$f" "$(basename "${f%.json}")"
done
bun "$ROOT/scripts/document-convert/schema-dump.mjs" >"$DIR/schema.json"
echo "regenerated $(ls "$OUT" | wc -l) yjs seed oracle cases in $OUT"
