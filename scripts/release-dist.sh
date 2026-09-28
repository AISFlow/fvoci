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
python3 - "$ROOT" <<'PY'
import json, os, re, sys
from pathlib import Path

root = Path(sys.argv[1])
env = os.environ
errors = []
if not re.fullmatch(r"0\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", env["VERSION"]):
    errors.append(f"version {env['VERSION']!r} is not 0.y.z")
if not re.fullmatch(r"[0-9a-f]{40}", env["SHA"]):
    errors.append("--sha must be a full commit SHA")
if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", env["REPOSITORY"]):
    errors.append("--repository must be owner/name")
if not re.fullmatch(r"[a-z0-9.-]+(/[a-z0-9._-]+)+", env["IMAGE"]):
    errors.append("--image must be a lowercase registry/repository without tag")
for key in ("INDEX_DIGEST", "AMD64_DIGEST", "ARM64_DIGEST"):
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", env[key]):
        errors.append(f"{key.lower()} must be sha256:<64 hex>")
if not env["RUN_URL"].startswith("https://"):
    errors.append("--run-url must be an https URL")
if errors:
    sys.exit("\n".join(errors))

out = Path(env["OUT"])
image_ref = f"{env['IMAGE']}:{env['VERSION']}@{env['INDEX_DIGEST']}"

compose_text = (root / env["COMPOSE_SOURCE"]).read_text(encoding="utf-8")
# The user compose names the product image once, as the YAML anchor every
# service on the product image shares:
#   x-fvoci-image: &fvoci-image ${FVOCI_IMAGE:-ghcr.io/aisflow/fvoci:<version>}
anchor = re.compile(
    r"^(x-fvoci-image:[ \t]+&fvoci-image[ \t]+)\$\{FVOCI_IMAGE:-" + re.escape(env["IMAGE"]) + r":[^}\s$]+\}[ \t]*$",
    re.MULTILINE,
)
anchors = anchor.findall(compose_text)
if len(anchors) != 1:
    sys.exit(f"{env['COMPOSE_SOURCE']}: expected exactly one "
             f"'x-fvoci-image: &fvoci-image ${{FVOCI_IMAGE:-{env['IMAGE']}:...}}' line, found {len(anchors)}")
if compose_text.count("FVOCI_IMAGE") != 1:
    sys.exit(f"{env['COMPOSE_SOURCE']}: FVOCI_IMAGE may appear only in the x-fvoci-image anchor")
rendered = anchor.sub(lambda m: m.group(1) + image_ref, compose_text)
if rendered.count(env["IMAGE"]) != 1:
    sys.exit(f"{env['COMPOSE_SOURCE']}: {env['IMAGE']} must be named only through the x-fvoci-image anchor")
if not re.search(r"^\s+image:[ \t]+\*fvoci-image[ \t]*$", rendered, re.MULTILINE):
    sys.exit(f"{env['COMPOSE_SOURCE']}: no service uses 'image: *fvoci-image'")
# Everything left for Compose to interpolate comes from the user's .env: each
# variable is required (${VAR:?message}, so an unfilled .env stops Compose
# before any container starts) and .env.example assigns exactly these
# variables. No env_file: each service gets only the values it names.
env_text = (root / env["ENV_SOURCE"]).read_text(encoding="utf-8")
assigned = re.findall(r"^([A-Z][A-Z0-9_]*)=", env_text, re.MULTILINE)
if len(assigned) != len(set(assigned)):
    sys.exit(f"{env['ENV_SOURCE']}: a variable is assigned twice")
references = re.findall(r"(?<!\$)\$(?!\$)(\{[^}]*\}|[A-Za-z_][A-Za-z0-9_]*)", rendered)
used = set()
for ref in references:
    match = re.fullmatch(r"\{([A-Z][A-Z0-9_]*):\?[^}]+\}", ref)
    if not match:
        sys.exit(f"{env['COMPOSE_SOURCE']}: every interpolation must be ${{VAR:?message}}; found ${ref}")
    used.add(match.group(1))
# Compose file secrets read their variable without interpolation; each such
# variable must also be required above, or an unfilled .env would reach a
# container as an empty secret.
secret_sources = set(re.findall(r"^    environment:[ \t]+([A-Z][A-Z0-9_]*)[ \t]*$", rendered, re.MULTILINE))
if secret_sources - used:
    sys.exit(f"{env['COMPOSE_SOURCE']}: secrets read {sorted(secret_sources - used)} without a ${{VAR:?message}} check")
if used != set(assigned):
    sys.exit(f"{env['ENV_SOURCE']} must assign exactly the variables {env['COMPOSE_SOURCE']} reads: "
             f"missing {sorted(used - set(assigned))}, unused {sorted(set(assigned) - used)}")
if re.search(r"^\s+env_file:", rendered, re.MULTILINE):
    sys.exit(f"{env['COMPOSE_SOURCE']}: the release compose must not use env_file")
header = (
    f"# FVOCI {env['VERSION']} ({env['SHA']}), rendered from {env['COMPOSE_SOURCE']}\n"
    f"# by the release workflow. The image is pinned by manifest digest.\n"
)
(out / "compose.yml").write_text(header + rendered, encoding="utf-8")
(out / "env.example").write_text(env_text, encoding="utf-8")
guide = (root / env["GUIDE_SOURCE"]).read_text(encoding="utf-8")
(out / "INSTALL.md").write_text(f"<!-- FVOCI {env['VERSION']} ({env['SHA']}) -->\n" + guide, encoding="utf-8")

record = {
    "version": env["VERSION"],
    "tag": f"v{env['VERSION']}",
    "sourceSha": env["SHA"],
    "image": image_ref,
    "indexDigest": env["INDEX_DIGEST"],
    "platforms": {"linux/amd64": env["AMD64_DIGEST"], "linux/arm64": env["ARM64_DIGEST"]},
    "composeSource": env["COMPOSE_SOURCE"],
    "files": ["compose.yml", "env.example", "INSTALL.md"],
    # Publish order: the index is pushed by digest only; both smoke jobs pull
    # that digest anonymously; then the publish job applies the immutable
    # version tag and, when this is the newest 0.y release, moves the minor tag;
    # the GitHub release comes last.
    "tags": {"immutable": env["VERSION"], "floating": env["VERSION"].rsplit(".", 1)[0]},
    "publishOrder": ["index-by-digest", "smoke-linux/amd64", "smoke-linux/arm64",
                     f"tag:{env['VERSION']}", f"tag:{env['VERSION'].rsplit('.', 1)[0]} (if newest)",
                     "github-release"],
}
(out / "release.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")

notes = (root / "scripts/release-notes-template.md").read_text(encoding="utf-8")
for key, value in {
    "@VERSION@": env["VERSION"],
    "@SHA@": env["SHA"],
    "@REPOSITORY@": env["REPOSITORY"],
    "@IMAGE_REF@": image_ref,
    "@AMD64_DIGEST@": env["AMD64_DIGEST"],
    "@ARM64_DIGEST@": env["ARM64_DIGEST"],
    "@RUN_URL@": env["RUN_URL"],
}.items():
    notes = notes.replace(key, value)
left = sorted(set(re.findall(r"@[A-Z0-9_]+@", notes)))
if left:
    sys.exit(f"release notes placeholders left: {left}")
(out / "RELEASE-NOTES.md").write_text(notes, encoding="utf-8")
PY

(cd "$OUT" && sha256sum compose.yml env.example INSTALL.md release.json RELEASE-NOTES.md >SHA256SUMS)
echo "rendered $OUT from $COMPOSE_SOURCE"
