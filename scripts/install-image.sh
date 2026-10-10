#!/usr/bin/env bash
# Builds the infra/rust install image once and hands it to the container smokes.
#
#   scripts/install-image.sh build [--tag REF]       build + label from this checkout
#   scripts/install-image.sh verify REF [ID]         refuse an image not built from this checkout
#   scripts/install-image.sh save REF DIR            DIR/image.tar + DIR/image.manifest
#   scripts/install-image.sh load DIR                check, docker load, verify
#
# build, verify and load print `FVOCI_INSTALL_IMAGE=<ref>` and
# `FVOCI_INSTALL_IMAGE_ID=<id>` on stdout (CI appends them to $GITHUB_ENV);
# everything else goes to stderr. Identity labels, all compared on verify:
#   io.fvoci.install-image.source-tree        tree of this checkout (HEAD^{tree} when
#                                             clean; else tracked + untracked, non-ignored)
#   io.fvoci.install-image.arch               Docker daemon architecture (also the image's)
#   io.fvoci.install-image.dockerfile-sha256  infra/rust/Dockerfile + .dockerignore
#   io.fvoci.install-image.toolchain          rust-toolchain.toml channel + .bun-version
# Recorded only: source-commit (HEAD) and builder (Docker engine/buildx), which
# are not image inputs. A mismatch is refused; nothing here rebuilds on refusal.
# Limit: git-ignored files that .dockerignore does not exclude also reach the
# build context but not the source-tree hash; CI builds from a fresh checkout,
# which has none.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LABEL=io.fvoci.install-image
DEFAULT_TAG=fvoci-rust-install:local

die() {
  printf 'install-image: %s\n' "$*" >&2
  exit 1
}

usage() {
  sed -n '2,7p' "${BASH_SOURCE[0]}" >&2
  exit 2
}

# Prints the tree id; every step returns its failure (no errexit in $(...)).
source_tree() {
  local status index tree
  status="$(git -C "$ROOT" status --porcelain --untracked-files=normal)" || return 1
  if [[ -z "$status" ]]; then
    git -C "$ROOT" rev-parse 'HEAD^{tree}'
    return
  fi
  # Dirty tree: hash it through a throwaway index so the real index is untouched.
  index="$(mktemp "${TMPDIR:-/tmp}/fvoci-install-image-index.XXXXXX")" || return 1
  cp "$(git -C "$ROOT" rev-parse --path-format=absolute --git-path index)" "$index" \
    && GIT_INDEX_FILE="$index" git -C "$ROOT" add -A \
    && tree="$(GIT_INDEX_FILE="$index" git -C "$ROOT" write-tree)"
  local rc=$?
  rm -f "$index"
  (( rc == 0 )) || return "$rc"
  printf '%s\n' "$tree"
}

# EXPECTED[key]: the identity labels of this checkout, computed once in this
# shell so any failure stops the script (and an empty value is never compared).
declare -A EXPECTED=()
LABEL_KEYS=(source-tree arch dockerfile-sha256 toolchain)
compute_expected() {
  local value channel bun
  value="$(source_tree)" || die "cannot hash the source tree of $ROOT"
  [[ "$value" =~ ^[0-9a-f]{40,64}$ ]] || die "unexpected source tree id: $value"
  EXPECTED[source-tree]="$value"
  value="$(docker version -f '{{.Server.Arch}}')" || die "cannot read the Docker daemon architecture"
  [[ -n "$value" ]] || die "empty Docker daemon architecture"
  EXPECTED[arch]="$value"
  value="$(cat "$ROOT/infra/rust/Dockerfile" "$ROOT/.dockerignore" | sha256sum | cut -d' ' -f1)"
  [[ "$value" =~ ^[0-9a-f]{64}$ ]] || die "cannot hash infra/rust/Dockerfile and .dockerignore"
  EXPECTED[dockerfile-sha256]="$value"
  channel="$(sed -nE 's/^channel = "([^"]+)"$/\1/p' "$ROOT/rust-toolchain.toml")"
  bun="$(tr -d '[:space:]' <"$ROOT/.bun-version")"
  [[ -n "$channel" && -n "$bun" ]] || die "no channel in rust-toolchain.toml or no .bun-version"
  EXPECTED[toolchain]="rust=${channel};bun=${bun}"
}

emit() {
  printf 'FVOCI_INSTALL_IMAGE=%s\nFVOCI_INSTALL_IMAGE_ID=%s\n' "$1" "$2"
}

verify() {
  local ref="$1" want_id="${2:-}" fields key refused=0 template='{{.Id}}{{"\n"}}{{.Architecture}}'
  (( ${#EXPECTED[@]} )) || compute_expected
  for key in "${LABEL_KEYS[@]}" source-commit builder; do
    template+="{{\"\\n\"}}{{index .Config.Labels \"$LABEL.$key\"}}"
  done
  fields="$(docker image inspect -f "$template" "$ref" 2>/dev/null)" \
    || die "image not found: $ref (build it: bash scripts/install-image.sh build)"
  local -a got
  mapfile -t got <<<"$fields"
  local id="${got[0]}" arch="${got[1]}" i=2
  if [[ -n "$want_id" && "$id" != "$want_id" ]]; then
    die "refusing $ref: image ID $id, expected $want_id"
  fi
  for key in "${LABEL_KEYS[@]}"; do
    if [[ "${got[i]:-}" != "${EXPECTED[$key]}" ]]; then
      printf 'install-image: refusing %s: label %s.%s=%q, this checkout has %q\n' \
        "$ref" "$LABEL" "$key" "${got[i]:-}" "${EXPECTED[$key]}" >&2
      refused=1
    fi
    i=$((i + 1))
  done
  if [[ "$arch" != "${EXPECTED[arch]}" ]]; then
    printf 'install-image: refusing %s: image architecture %s, daemon %s\n' "$ref" "$arch" "${EXPECTED[arch]}" >&2
    refused=1
  fi
  (( refused == 0 )) || die "refusing $ref; rebuild it from this checkout: bash scripts/install-image.sh build --tag $ref"
  printf 'install-image: verified %s id=%s commit=%s builder=%s\n' "$ref" "$id" "${got[i]:-}" "${got[i + 1]:-}" >&2
  emit "$ref" "$id"
}

build() {
  local tag="$DEFAULT_TAG" labels=() key commit engine buildx started=$SECONDS
  while (($#)); do
    case "$1" in
      --tag) tag="${2:?}"; shift 2 ;;
      *) usage ;;
    esac
  done
  compute_expected
  for key in "${LABEL_KEYS[@]}"; do
    labels+=(--label "$LABEL.$key=${EXPECTED[$key]}")
  done
  commit="$(git -C "$ROOT" rev-parse HEAD)" || die "cannot read HEAD"
  engine="$(docker version -f '{{.Server.Version}}')" || die "cannot read the Docker engine version"
  buildx="$(docker buildx version | cut -d' ' -f1-2)" || die "cannot read the buildx version"
  labels+=(--label "$LABEL.source-commit=$commit" --label "org.opencontainers.image.revision=$commit")
  labels+=(--label "$LABEL.builder=docker ${engine}; ${buildx}")
  printf 'install-image: building %s from %s\n' "$tag" "$ROOT" >&2
  docker build -f "$ROOT/infra/rust/Dockerfile" "${labels[@]}" -t "$tag" "$ROOT" >&2
  printf 'install-image: built %s in %ss\n' "$tag" "$((SECONDS - started))" >&2
  verify "$tag"
}

# Manifest lines are `key=value`; values never contain newlines.
manifest_get() {
  local file="$1" key="$2" line value="" n=0
  while IFS= read -r line; do
    if [[ "$line" == "$key="* ]]; then
      value="${line#*=}"
      n=$((n + 1))
    fi
  done <"$file"
  (( n == 1 )) || die "manifest $file: expected one $key, found $n"
  printf '%s\n' "$value"
}

save() {
  local ref="$1" dir="$2" out id
  out="$(verify "$ref")"
  id="$(sed -n 's/^FVOCI_INSTALL_IMAGE_ID=//p' <<<"$out")"
  mkdir -p "$dir"
  local started=$SECONDS
  docker save -o "$dir/image.tar" "$ref"
  {
    printf 'ref=%s\nid=%s\n' "$ref" "$id"
    printf 'tar-sha256=%s\n' "$(sha256sum "$dir/image.tar" | cut -d' ' -f1)"
    printf 'repository=%s\nrun-id=%s\nrun-attempt=%s\n' \
      "${GITHUB_REPOSITORY:-}" "${GITHUB_RUN_ID:-}" "${GITHUB_RUN_ATTEMPT:-}"
  } >"$dir/image.manifest"
  printf 'install-image: saved %s (%s bytes) in %ss\n' "$ref" "$(stat -c %s "$dir/image.tar")" "$((SECONDS - started))" >&2
  emit "$ref" "$id"
}

load() {
  local dir="$1" manifest="$1/image.manifest" ref id key
  [[ -f "$manifest" && -f "$dir/image.tar" ]] || die "missing $dir/image.tar or image.manifest"
  ref="$(manifest_get "$manifest" ref)"
  id="$(manifest_get "$manifest" id)"
  # The producer must be this repository, run and attempt (no foreign or stale artifact).
  for key in repository:GITHUB_REPOSITORY run-id:GITHUB_RUN_ID run-attempt:GITHUB_RUN_ATTEMPT; do
    local name="${key%%:*}" var="${key#*:}" recorded
    recorded="$(manifest_get "$manifest" "$name")"
    [[ "$recorded" == "${!var:-}" ]] || die "manifest $name=$recorded, this job has ${!var:-}"
  done
  [[ "$(sha256sum "$dir/image.tar" | cut -d' ' -f1)" == "$(manifest_get "$manifest" tar-sha256)" ]] \
    || die "image.tar sha256 does not match the manifest"
  local started=$SECONDS loaded
  loaded="$(docker load -i "$dir/image.tar")"
  printf '%s\n' "$loaded" >&2
  grep -Fqx "Loaded image: $ref" <<<"$loaded" || die "docker load did not load $ref"
  printf 'install-image: loaded %s in %ss\n' "$ref" "$((SECONDS - started))" >&2
  verify "$ref" "$id"
}

(($#)) || usage
cmd="$1"
shift
case "$cmd" in
  build) build "$@" ;;
  verify) (($# == 1 || $# == 2)) || usage; verify "$@" ;;
  save) (($# == 2)) || usage; save "$@" ;;
  load) (($# == 1)) || usage; load "$@" ;;
  *) usage ;;
esac
