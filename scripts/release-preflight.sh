#!/usr/bin/env bash
# Cheap checks the release workflow runs before anything is built or pushed
# (docs/RELEASING.md), so a tag that cannot pass the smoke fails early:
#   - the release notes were rewritten for this version (no TODO(release)
#     marker, and a "<!-- notes-for: 0.y.z -->" line naming it);
#   - the rust-build stage of infra/rust/Dockerfile declares
#     ARG FVOCI_BUILD_SHA, so fvoci-server --version reports the commit;
#   - the user compose renders through scripts/release-dist.sh (the
#     x-fvoci-image anchor and env.example); from an empty directory with an
#     empty environment, `docker compose config` refuses the unfilled
#     env.example as .env and accepts it once every value is filled in; the
#     values Compose passes as secrets are in no service's environment, and
#     the app's secret files are root-only (uid 0, mode 0400).
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
cp "$WORK/dist/env.example" "$WORK/empty/.env"
if (cd "$WORK/empty" && env -i PATH="$PATH" HOME="$WORK" docker compose -f compose.yml config -q) 2>/dev/null; then
  fail "docker compose config accepts the unfilled env.example; every value must be required"
fi
sed -E 's/^([A-Z][A-Z0-9_]*)=$/\1=preflight-\1/' "$WORK/dist/env.example" >"$WORK/empty/.env"
(cd "$WORK/empty" && env -i PATH="$PATH" HOME="$WORK" docker compose -f compose.yml config --format json) >"$WORK/config.json" \
  || fail "docker compose config rejects the rendered user compose with a filled .env"
python3 - "$WORK/config.json" "$IMAGE:$VERSION@$ZERO" <<'PY' || fail "the rendered user compose does not match the release contract (docs/RELEASING.md)"
import json, sys
config = json.load(open(sys.argv[1]))
services = config["services"]
image = sys.argv[2]
product = sorted(name for name, spec in services.items() if spec.get("image") == image)
apps = sorted(name for name, spec in services.items()
              if any(p.get("target") == 8080 for p in spec.get("ports") or []))
problems = []
if len(apps) != 1:
    problems.append(f"expected one service publishing container port 8080, found {apps}")
elif apps[0] not in product:
    problems.append(f"{apps[0]} (publishes 8080) does not use the product image")
if "postgres" not in services:
    problems.append("no postgres service")
for name, spec in services.items():
    if spec.get("env_file"):
        problems.append(f"{name} needs an env_file")
secret_values = {f"preflight-{s['environment']}" for s in config.get("secrets", {}).values() if s.get("environment")}
for name, spec in services.items():
    leaked = sorted(k for k, v in (spec.get("environment") or {}).items() if v in secret_values)
    if leaked:
        problems.append(f"{name} gets secret values as environment: {leaked}")
for app in apps:
    for mount in services[app].get("secrets") or []:
        if str(mount.get("uid")) != "0" or str(mount.get("mode")) not in ("256", "0400", "400"):
            problems.append(f"{app} secret {mount.get('source')} is not root-only (uid 0, mode 0400)")
print(f"product image services: {product}; app: {apps}")
sys.exit("\n".join(problems) if problems else 0)
PY
echo "rendered user compose: unfilled env.example refused, filled .env accepted by docker compose config, secrets only as root-only files: ok"
