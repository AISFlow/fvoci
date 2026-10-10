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

source_tree() {
  if [[ -z "$(git -C "$ROOT" status --porcelain --untracked-files=normal)" ]]; then
    git -C "$ROOT" rev-parse 'HEAD^{tree}'
    return
  fi
  # Dirty tree: hash it through a throwaway index so the real index is untouched.
  local index
  index="$(mktemp "${TMPDIR:-/tmp}/fvoci-install-image-index.XXXXXX")"
  cp "$(git -C "$ROOT" rev-parse --path-format=absolute --git-path index)" "$index"
  GIT_INDEX_FILE="$index" git -C "$ROOT" add -A
  GIT_INDEX_FILE="$index" git -C "$ROOT" write-tree
  rm -f "$index"
}

daemon_arch() {
  docker version -f '{{.Server.Arch}}'
}

dockerfile_sha256() {
  cat "$ROOT/infra/rust/Dockerfile" "$ROOT/.dockerignore" | sha256sum | cut -d' ' -f1
}

toolchain() {
  local channel
  channel="$(sed -nE 's/^channel = "([^"]+)"$/\1/p' "$ROOT/rust-toolchain.toml")"
  [[ -n "$channel" ]] || die "no channel in rust-toolchain.toml"
  printf 'rust=%s;bun=%s\n' "$channel" "$(tr -d '[:space:]' <"$ROOT/.bun-version")"
}

# Expected labels for this checkout, one `key=value` per line.
expected_labels() {
  printf 'source-tree=%s\n' "$(source_tree)"
  printf 'arch=%s\n' "$(daemon_arch)"
  printf 'dockerfile-sha256=%s\n' "$(dockerfile_sha256)"
  printf 'toolchain=%s\n' "$(toolchain)"
}

image_label() {
  docker image inspect -f "{{index .Config.Labels \"$LABEL.$2\"}}" "$1"
}

emit() {
  printf 'FVOCI_INSTALL_IMAGE=%s\nFVOCI_INSTALL_IMAGE_ID=%s\n' "$1" "$2"
}

verify() {
  local ref="$1" want_id="${2:-}" id arch key want got refused=0
  id="$(docker image inspect -f '{{.Id}}' "$ref" 2>/dev/null)" || die "image not found: $ref (build it: bash scripts/install-image.sh build)"
  if [[ -n "$want_id" && "$id" != "$want_id" ]]; then
    die "refusing $ref: image ID $id, expected $want_id"
  fi
  arch="$(docker image inspect -f '{{.Architecture}}' "$ref")"
  while IFS='=' read -r key want; do
    got="$(image_label "$ref" "$key")"
    if [[ "$got" != "$want" ]]; then
      printf 'install-image: refusing %s: label %s.%s=%q, this checkout has %q\n' "$ref" "$LABEL" "$key" "$got" "$want" >&2
      refused=1
    fi
  done < <(expected_labels)
  if [[ "$arch" != "$(daemon_arch)" ]]; then
    printf 'install-image: refusing %s: image architecture %s, daemon %s\n' "$ref" "$arch" "$(daemon_arch)" >&2
    refused=1
  fi
  (( refused == 0 )) || die "refusing $ref; rebuild it from this checkout: bash scripts/install-image.sh build --tag $ref"
  printf 'install-image: verified %s id=%s commit=%s builder=%s\n' "$ref" "$id" \
    "$(image_label "$ref" source-commit)" "$(image_label "$ref" builder)" >&2
  emit "$ref" "$id"
}

build() {
  local tag="$DEFAULT_TAG" labels=() key value started=$SECONDS
  while (($#)); do
    case "$1" in
      --tag) tag="${2:?}"; shift 2 ;;
      *) usage ;;
    esac
  done
  while IFS='=' read -r key value; do
    labels+=(--label "$LABEL.$key=$value")
  done < <(expected_labels)
  local commit
  commit="$(git -C "$ROOT" rev-parse HEAD)"
  labels+=(--label "$LABEL.source-commit=$commit" --label "org.opencontainers.image.revision=$commit")
  labels+=(--label "$LABEL.builder=docker $(docker version -f '{{.Server.Version}}'); $(docker buildx version 2>/dev/null | cut -d' ' -f1-2)")
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
