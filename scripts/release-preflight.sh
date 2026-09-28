#!/usr/bin/env bash
# Cheap checks the release workflow runs before anything is built or pushed
# (docs/RELEASING.md), so a tag that cannot pass the smoke fails early:
#   - the release notes were rewritten for this version (no TODO(release)
#     marker, and a "<!-- notes-for: 0.y.z -->" line naming it);
#   - the rust-build stage of infra/rust/Dockerfile declares
#     ARG FVOCI_BUILD_SHA, so fvoci-server --version reports the commit;
#   - the user compose renders through scripts/release-dist.sh (the
#     x-fvoci-image anchor) and `docker compose config` accepts the result from
#     an empty directory with an empty environment.
#
#   scripts/release-preflight.sh --version 0.y.z [--image ghcr.io/aisflow/fvoci]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="" IMAGE="ghcr.io/aisflow/fvoci"
while (($#)); do
  case "$1" in
    --version) VERSION="$2" ;;
    --image) IMAGE="$2" ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift 2
done
[[ "$VERSION" =~ ^0\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || { echo "--version must be 0.y.z" >&2; exit 2; }
fail() { echo "release-preflight: $*" >&2; exit 1; }

NOTES="$ROOT/scripts/release-notes-template.md"
if grep -n 'TODO(release)' "$NOTES" >&2; then
  fail "scripts/release-notes-template.md still has TODO(release) markers for ${VERSION}"
fi
grep -qxF "<!-- notes-for: ${VERSION} -->" "$NOTES" \
  || fail "scripts/release-notes-template.md is not marked '<!-- notes-for: ${VERSION} -->'; rewrite the notes for this version"
echo "release notes written for ${VERSION}: ok"

python3 - "$ROOT/infra/rust/Dockerfile" <<'PY' || fail "infra/rust/Dockerfile: the rust-build stage must declare ARG FVOCI_BUILD_SHA"
import re, sys
stage = None
for line in open(sys.argv[1], encoding="utf-8"):
    start = re.match(r"\s*FROM\s+\S+(?:\s+AS\s+(\S+))?", line, re.IGNORECASE)
    if start:
        stage = (start.group(1) or "").lower()
    elif stage == "rust-build" and re.match(r"\s*ARG\s+FVOCI_BUILD_SHA(=|\s|$)", line):
        sys.exit(0)
sys.exit(1)
PY
echo "Dockerfile rust-build stage declares ARG FVOCI_BUILD_SHA: ok"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
ZERO="sha256:$(printf '0%.0s' {1..64})"
bash "$ROOT/scripts/release-dist.sh" --version "$VERSION" --sha "$(printf 'f%.0s' {1..40})" \
  --repository preflight/check --image "$IMAGE" --index-digest "$ZERO" \
  --amd64-digest "$ZERO" --arm64-digest "$ZERO" --run-url https://preflight.invalid/ --out "$WORK/dist" >/dev/null
mkdir "$WORK/empty"
cp "$WORK/dist/compose.yml" "$WORK/empty/compose.yml"
(cd "$WORK/empty" && env -i PATH="$PATH" HOME="$WORK" docker compose -f compose.yml config --format json) >"$WORK/config.json" \
  || fail "docker compose config rejects the rendered user compose"
python3 - "$WORK/config.json" "$IMAGE:$VERSION@$ZERO" <<'PY' || fail "the rendered user compose does not match the release contract (docs/RELEASING.md)"
import json, sys
services = json.load(open(sys.argv[1]))["services"]
image = sys.argv[2]
product = sorted(name for name, spec in services.items() if spec.get("image") == image)
one_shot = sorted({dep for spec in services.values()
                   for dep, cond in (spec.get("depends_on") or {}).items()
                   if cond.get("condition") == "service_completed_successfully"})
apps = sorted(name for name, spec in services.items()
              if any(p.get("target") == 8080 for p in spec.get("ports") or []))
problems = []
if len(apps) != 1:
    problems.append(f"expected one service publishing container port 8080, found {apps}")
elif apps[0] not in product:
    problems.append(f"{apps[0]} (publishes 8080) does not use the product image")
if not one_shot or not set(one_shot) <= set(product):
    problems.append(f"one-shot services {one_shot} must exist and use the product image")
if "postgres" not in services:
    problems.append("no postgres service")
for name, spec in services.items():
    if spec.get("env_file"):
        problems.append(f"{name} needs an env_file")
print(f"product image services: {product}; app: {apps}; one-shot: {one_shot}")
sys.exit("\n".join(problems) if problems else 0)
PY
echo "rendered user compose passes docker compose config with an empty environment: ok"
