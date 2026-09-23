#!/bin/sh
# Materialize edwardkim/rhwp at the Cargo.toml metadata pin.
# Sparse only the library + workspace member crates required to load the path
# package. Does not copy editor/samples/npm or llm_verifier fixture trees.
# Refuses a dirty or mismatched existing checkout. Does not delete it.
set -eu
ROOT="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
META="$ROOT/Cargo.toml"
DEST="${RHWP_DEST:-$ROOT/.vendor-src/rhwp}"

REV="$(sed -n 's/^rhwp_rev = "\([^"]*\)"/\1/p' "$META" | head -n 1)"
REPO="$(sed -n 's/^rhwp_repo = "\([^"]*\)"/\1/p' "$META" | head -n 1)"
if [ -z "$REV" ] || [ -z "$REPO" ]; then
  echo "refuse: rhwp_rev/rhwp_repo missing from $META metadata" >&2
  exit 1
fi
if [ -n "${RHWP_REV:-}" ] && [ "$RHWP_REV" != "$REV" ]; then
  echo "refuse: RHWP_REV=$RHWP_REV does not match metadata $REV" >&2
  exit 1
fi
if [ -n "${RHWP_REPO:-}" ]; then
  case "$RHWP_REPO" in
    "$REPO"|"$REPO.git"|"$REPO/") ;;
    *)
      echo "refuse: RHWP_REPO=$RHWP_REPO does not match metadata $REPO" >&2
      exit 1
      ;;
  esac
fi
FETCH_REPO="$REPO.git"
case "$REPO" in
  *.git) FETCH_REPO="$REPO" ;;
esac

origin_ok() {
  case "$1" in
    "$REPO"|"$REPO.git"|"$REPO/"|"$REPO.git/") return 0 ;;
    *) return 1 ;;
  esac
}

apply_sparse() {
  git -C "$DEST" sparse-checkout init --no-cone
  git -C "$DEST" sparse-checkout set \
    /src/ \
    /crates/ \
    /bindings/Native/ \
    /tools/rhwp-subsecond/ \
    /tools/batch-convert/ \
    /tools/llm_verifier/verdict_protocol/ \
    /tools/llm_verifier/claim_bind/src/ \
    /tools/llm_verifier/claim_bind/Cargo.toml \
    /tools/llm_verifier/criteria_decomp/Cargo.toml \
    /tools/llm_verifier/criteria_decomp/src/ \
    /Cargo.toml \
    /Cargo.lock \
    /LICENSE \
    /build.rs \
    /rust-toolchain.toml \
    /THIRD_PARTY_LICENSES.md \
    /saved/blank2010.hwp
}

verify_existing() {
  origin="$(git -C "$DEST" remote get-url origin)"
  if ! origin_ok "$origin"; then
    echo "refuse: origin is $origin, expected $REPO; leaving $DEST untouched" >&2
    exit 1
  fi
  if [ -n "$(git -C "$DEST" status --porcelain)" ]; then
    echo "refuse: dirty checkout at $DEST; leaving it untouched" >&2
    git -C "$DEST" status --porcelain >&2
    exit 1
  fi
  got="$(git -C "$DEST" rev-parse HEAD)"
  if [ "$got" != "$REV" ]; then
    echo "refuse: HEAD $got != metadata $REV; leaving $DEST untouched" >&2
    exit 1
  fi
}

if [ -e "$DEST" ]; then
  if [ ! -d "$DEST/.git" ]; then
    echo "refuse: $DEST exists and is not a git checkout; leaving it untouched" >&2
    exit 1
  fi
  verify_existing
  apply_sparse
  git -C "$DEST" checkout --detach HEAD >/dev/null
  verify_existing
  echo "rhwp $(git -C "$DEST" rev-parse HEAD) already pinned at $DEST"
  exit 0
fi

mkdir -p "$(dirname "$DEST")"
git init "$DEST"
git -C "$DEST" remote add origin "$FETCH_REPO"
git -C "$DEST" fetch --depth 1 --filter=blob:none origin "$REV"
apply_sparse
git -C "$DEST" checkout --detach FETCH_HEAD
got="$(git -C "$DEST" rev-parse HEAD)"
if [ "$got" != "$REV" ]; then
  echo "refuse: fetched $got != metadata $REV; leaving $DEST as fetched" >&2
  exit 1
fi
if [ -n "$(git -C "$DEST" status --porcelain)" ]; then
  echo "refuse: checkout dirty after fetch; leaving $DEST untouched" >&2
  exit 1
fi
echo "rhwp $got ready at $DEST"
