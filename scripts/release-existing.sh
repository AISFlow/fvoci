#!/usr/bin/env bash
# Idempotency guard for a release re-run. Prints the image index digest that
# this version already carries (empty when it has none yet), so the workflow
# reuses it instead of building again:
#   - a GitHub release for the tag must carry release.json for the same source
#     SHA, and the registry tag must still point at the digest it records;
#   - a registry tag without a release (a run that tagged after its smoke but
#     stopped before the release job) is reused only when its OCI labels name
#     this version and SHA.
# The :0.y.z tag is only applied after the smoke passed (docs/RELEASING.md), so
# an index pushed by digest for a run whose smoke failed is never reused.
# Decisions use exit codes and JSON (tools/release/release-api.ts), not error text.
# Anything else fails closed; this never moves a tag.
# Needs GH_TOKEN (contents: read), GITHUB_REPOSITORY, REGISTRY_USER/PASSWORD
# (packages: read) and a docker registry login for the label check.
#
#   scripts/release-existing.sh --tag v0.y.z --version 0.y.z --sha <sha> --image ghcr.io/aisflow/fvoci
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAG="" VERSION="" SHA="" IMAGE=""
while (($#)); do
  case "$1" in
    --tag) TAG="$2" ;;
    --version) VERSION="$2" ;;
    --sha) SHA="$2" ;;
    --image) IMAGE="$2" ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift 2
done
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
fail() { echo "release-existing: $*" >&2; exit 1; }
API=(bun "$ROOT/tools/release/release-api.ts")
JSON=(bun "$ROOT/tools/release/release-json.ts")

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

STATE="$("${API[@]}" release-state --tag "$TAG")"
RECORD_DIGEST=""
RELEASE_STATE="$("${JSON[@]}" state "$STATE")"
case "$RELEASE_STATE" in
  none) ;;
  published)
    gh release download "$TAG" --repo "$GITHUB_REPOSITORY" --pattern release.json --dir "$WORK" \
      || fail "release $TAG exists without release.json; inspect it and delete it by hand before re-running"
    RECORD_DIGEST="$("${JSON[@]}" record-digest "$WORK/release.json" "$VERSION" "$SHA")"
    ;;
  *) fail "release $TAG is a draft; inspect it and delete it by hand before re-running" ;;
esac

REGISTRY_DIGEST=""
set +e
REGISTRY_DIGEST="$("${API[@]}" registry-digest --image "$IMAGE" --tag "$VERSION")"
status=$?
set -e
case "$status" in
  0) ;;
  4)
    # The job token cannot read the package; the first release has no package
    # yet. Without a release there is no recorded digest to protect here, and
    # the publish job re-checks the tag with its write token before tagging.
    [[ -z "$RECORD_DIGEST" ]] || fail "release $TAG records $RECORD_DIGEST but $IMAGE is not readable"
    echo "release-existing: $IMAGE not readable with this token; treating $VERSION as untagged" >&2
    ;;
  *) fail "could not inspect $IMAGE:$VERSION" ;;
esac

if [[ -n "$RECORD_DIGEST" && "$RECORD_DIGEST" != "$REGISTRY_DIGEST" ]]; then
  fail "release $TAG records $RECORD_DIGEST but $IMAGE:$VERSION is '${REGISTRY_DIGEST:-missing}'"
fi

if [[ -n "$REGISTRY_DIGEST" ]]; then
  docker buildx imagetools inspect "$IMAGE@$REGISTRY_DIGEST" --format '{{json .Image}}' >"$WORK/image.json"
  "${JSON[@]}" image-labels "$WORK/image.json" "$VERSION" "$SHA" \
    || fail "$IMAGE:$VERSION was not built from $SHA; refusing to reuse or overwrite it"
fi
printf '%s\n' "$REGISTRY_DIGEST"
