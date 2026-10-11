#!/usr/bin/env bash
# Render the release files for one image index pushed by digest
# (docs/RELEASING.md):
#   compose.yml    the user compose with its x-fvoci-image anchor
#                  ${FVOCI_IMAGE:-...} replaced by the digest-pinned image;
#                  never committed back
#   env.example    the settings the user copies to .env and fills in; it
#                  assigns exactly the variables compose.yml reads (no leading
#                  dot: GitHub renames release assets that start with one)
#   INSTALL.md     the short start guide
#   release.json   the release record (version, source SHA, digests)
#   RELEASE-NOTES.md
#   SHA256SUMS     over the five files above
#
#   scripts/release-dist.sh --version 0.y.z --sha <40-hex> --repository owner/name \
#     --image ghcr.io/aisflow/fvoci --index-digest sha256:... \
#     --amd64-digest sha256:... --arm64-digest sha256:... --run-url URL --out DIR
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="" SHA="" REPOSITORY="" IMAGE="" INDEX_DIGEST="" AMD64_DIGEST="" ARM64_DIGEST="" RUN_URL="" OUT=""
while (($#)); do
  case "$1" in
    --version) VERSION="$2" ;;
    --sha) SHA="$2" ;;
    --repository) REPOSITORY="$2" ;;
    --image) IMAGE="$2" ;;
    --index-digest) INDEX_DIGEST="$2" ;;
    --amd64-digest) AMD64_DIGEST="$2" ;;
    --arm64-digest) ARM64_DIGEST="$2" ;;
    --run-url) RUN_URL="$2" ;;
    --out) OUT="$2" ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift 2
done

# The standalone user compose lives at the first existing path and names the
# FVOCI image only through the x-fvoci-image anchor (checked below).
COMPOSE_SOURCE=""
for candidate in infra/rust/compose.user.yml infra/rust/compose.yml; do
  if [[ -f "$ROOT/$candidate" ]]; then
    COMPOSE_SOURCE="$candidate"
    break
  fi
done
if [[ -z "$COMPOSE_SOURCE" ]]; then
  echo "no user compose file found" >&2
  exit 1
fi
ENV_SOURCE="${COMPOSE_SOURCE%.yml}.env.example"
GUIDE_SOURCE="${COMPOSE_SOURCE%.yml}.INSTALL.md"
for source in "$ENV_SOURCE" "$GUIDE_SOURCE"; do
  [[ -f "$ROOT/$source" ]] || { echo "missing $source next to $COMPOSE_SOURCE" >&2; exit 1; }
done

mkdir -p "$OUT"
export VERSION SHA REPOSITORY IMAGE INDEX_DIGEST AMD64_DIGEST ARM64_DIGEST RUN_URL OUT COMPOSE_SOURCE ENV_SOURCE GUIDE_SOURCE
bun "$ROOT/tools/release/dist-check.ts" render "$ROOT"

(cd "$OUT" && sha256sum compose.yml env.example INSTALL.md release.json RELEASE-NOTES.md >SHA256SUMS)
echo "rendered $OUT from $COMPOSE_SOURCE"
