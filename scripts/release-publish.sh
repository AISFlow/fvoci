#!/usr/bin/env bash
# Create the GitHub pre-release for an existing tag from a rendered release
# directory (scripts/release-dist.sh). A re-run is a no-op when the tag
# already has a complete release for the same image digest; it never replaces
# assets and fails on any other existing release.
# Needs GH_TOKEN (contents: write) and GITHUB_REPOSITORY.
#
#   scripts/release-publish.sh --tag v0.y.z --dist DIR
set -euo pipefail

TAG="" DIST=""
while (($#)); do
  case "$1" in
    --tag) TAG="$2" ;;
    --dist) DIST="$2" ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift 2
done
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
fail() { echo "release-publish: $*" >&2; exit 1; }
ASSETS=(compose.yml SHA256SUMS release.json RELEASE-NOTES.md)

(cd "$DIST" && sha256sum --strict -c SHA256SUMS >/dev/null) || fail "SHA256SUMS does not match $DIST"
VERSION="$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); assert "v"+r["version"]==sys.argv[2], r; print(r["version"])' "$DIST/release.json" "$TAG")"
DIGEST="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["indexDigest"])' "$DIST/release.json")"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if gh release view "$TAG" --repo "$GITHUB_REPOSITORY" --json assets --jq '.assets[].name' >"$WORK/assets" 2>"$WORK/view.err"; then
  gh release download "$TAG" --repo "$GITHUB_REPOSITORY" --pattern release.json --dir "$WORK" \
    || fail "release $TAG exists without release.json; inspect and delete it by hand"
  RECORDED="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["indexDigest"])' "$WORK/release.json")"
  [[ "$RECORDED" == "$DIGEST" ]] || fail "release $TAG already records $RECORDED, not $DIGEST; refusing to overwrite"
  for asset in "${ASSETS[@]}"; do
    grep -qx "$asset" "$WORK/assets" || fail "release $TAG for $DIGEST lacks $asset; delete the partial release by hand and re-run"
  done
  echo "release $TAG already published for $DIGEST; nothing to do"
  exit 0
elif ! grep -qi 'release not found' "$WORK/view.err"; then
  cat "$WORK/view.err" >&2
  fail "could not read the GitHub release for $TAG"
fi

# --verify-tag: publish only for a tag that already exists on the remote; the
# workflow never creates or moves tags.
gh release create "$TAG" --repo "$GITHUB_REPOSITORY" --verify-tag --prerelease --latest=false \
  --title "FVOCI ${VERSION} (trial)" --notes-file "$DIST/RELEASE-NOTES.md" \
  "${ASSETS[@]/#/$DIST/}"

mkdir -p "$WORK/published"
gh release download "$TAG" --repo "$GITHUB_REPOSITORY" --dir "$WORK/published"
(cd "$WORK/published" && sha256sum --strict -c SHA256SUMS) || fail "published assets do not match SHA256SUMS"
cmp -s "$WORK/published/SHA256SUMS" "$DIST/SHA256SUMS" || fail "published SHA256SUMS differs from the rendered one"
echo "published pre-release $TAG for $DIGEST"
