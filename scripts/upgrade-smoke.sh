#!/usr/bin/env bash
# Image-to-image upgrade smoke for the infra/rust Compose install.
#
#   scripts/upgrade-smoke.sh --old <sha> --new <sha> [--main-ref origin/main] [--storage local|s3]
#                            [--evidence-dir DIR] [--build-jobs 2] [--min-free-gib 15] [--plan-only]
#
# --plan-only checks sources, migrations and recipes, and the disk gate for each
# image that would need a build, then exits without building, starting or
# tearing down anything.
# Both SHAs must be on the first-parent history of --main-ref, old an ancestor of
# new, and new must add at least two migrations. Each image is built from a
# `git archive` of its SHA (never the working tree) and labelled with it. The
# only recipe change is `ENV CARGO_BUILD_JOBS=<n>` in the Rust build stage.
# Matching labelled images are reused and kept afterwards.
#
# Flow: seed a project on the old image, back it up with the old checkout's
# backup.sh (--leave-stopped), block the newest migration so the new image's init
# fails, check the server stays stopped with the earlier migrations committed,
# remove the blocker, rerun `up` once, verify the data on the new image, stop
# it, then restore the pre-upgrade backup with the old checkout's restore.sh
# into a fresh project on the old image. The old image never runs on the
# migrated database.
#
# --storage s3 adds infra/rust/compose.s3.yml and follows RUNNING.md "S3 storage
# backup" instead of backup.sh/restore.sh (which must refuse S3): versioning on
# the project's own silo bucket, the documented server stop and a quiesced
# pg_dump. After the upgrade it deletes one stored object and overwrites another
# in the bucket, restores the dump into a fresh old-image project pointed at the
# same bucket, requires --verify-storage to refuse before the server starts,
# restores both objects from bucket versions, and only then starts the server.
#
# On success the trap tears both projects down with the compose file of the tree
# each was started from and checks that no container, volume or network of
# either project remains; only then does it remove the work dir. A failed
# teardown or a leftover fails the run and keeps the work dir. On any other
# failure it keeps the projects and work dir for diagnosis and prints the
# cleanup commands. The evidence dir (default: a new 0700 dir under TMPDIR)
# holds redacted logs only and is kept on success and failure.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OLD_REF=""
NEW_REF=""
MAIN_REF="origin/main"
EVIDENCE_DIR=""
BUILD_JOBS=2
MIN_FREE_GIB=15
PLAN_ONLY=0
STORAGE=local

usage() {
  sed -n '2,9p' "${BASH_SOURCE[0]}" >&2
  exit 2
}

while (($#)); do
  case "$1" in
    --old) OLD_REF="${2:?}"; shift 2 ;;
    --new) NEW_REF="${2:?}"; shift 2 ;;
    --main-ref) MAIN_REF="${2:?}"; shift 2 ;;
    --evidence-dir) EVIDENCE_DIR="${2:?}"; shift 2 ;;
    --build-jobs) BUILD_JOBS="${2:?}"; shift 2 ;;
    --min-free-gib) MIN_FREE_GIB="${2:?}"; shift 2 ;;
    --plan-only) PLAN_ONLY=1; shift ;;
    --storage) STORAGE="${2:?}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n "$OLD_REF" && -n "$NEW_REF" ]] || usage
[[ "$BUILD_JOBS" =~ ^[1-9][0-9]*$ && "$MIN_FREE_GIB" =~ ^[0-9]+$ ]] || usage
[[ "$STORAGE" == local || "$STORAGE" == s3 ]] || usage

require_cmd() {
  for cmd in "$@"; do
    command -v "$cmd" >/dev/null 2>&1 || {
      echo "missing required command: $cmd" >&2
      exit 1
    }
  done
}
require_cmd git docker openssl curl node python3 sha256sum tar awk diff

RUN_ID="$(openssl rand -hex 8)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-upgrade.${RUN_ID}.XXXXXX")"
chmod 700 "$WORK"
if [[ -z "$EVIDENCE_DIR" ]]; then
  EVIDENCE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-upgrade-evidence.${RUN_ID}.XXXXXX")"
fi
mkdir -p "$EVIDENCE_DIR"
ASSERT_LOG="$EVIDENCE_DIR/assertions.log"
: >"$ASSERT_LOG"
UPGRADE_PROJECT="fvoci-up-src-${RUN_ID}"
ROLLBACK_PROJECT="fvoci-up-rb-${RUN_ID}"
UPGRADE_ENV="$WORK/upgrade.env"
BACKUP_ENV="$WORK/backup.env"
ROLLBACK_ENV="$WORK/rollback.env"
# S3 only: joins the rollback server to the upgrade project's network so it
# reads the original bucket (RUNNING.md "S3 storage backup", item 3).
RB_BUCKET_OVERLAY="$WORK/rollback-bucket.yml"
BACKUP_DIR="$WORK/backup"
COOKIE_JAR="$WORK/cookies"
DOWNLOAD_PATH="$WORK/download"
FIXTURE_HWPX="$ROOT/compat/fixtures/sample.hwpx"
OLD_TREE="$WORK/src-old"
NEW_TREE="$WORK/src-new"
# The tree whose compose file last started the upgrade project.
UPGRADE_TREE="$OLD_TREE"
SECRETS=()
STACK_STARTED=0
START_TS=$SECONDS

log_assert() {
  printf '%s\n' "$1" | tee -a "$ASSERT_LOG"
}

fail() {
  echo "FAIL: $*" >&2
  printf 'FAIL: %s\n' "$*" >>"$ASSERT_LOG"
  exit 1
}

# Replace every generated secret with a marker before a log reaches the evidence dir.
# The secrets go through the environment, not argv, so ps does not list them.
redact() {
  FVOCI_REDACT="$(printf '%s\n' "${SECRETS[@]}")" python3 -c '
import os, sys
data = sys.stdin.read()
for secret in os.environ["FVOCI_REDACT"].split("\n"):
    if secret:
        data = data.replace(secret, "[redacted]")
sys.stdout.write(data)
'
}

project_compose() {
  local project="$1" env_file="$2" tree="$3"
  shift 3
  local files=(-f "$tree/infra/rust/compose.yml")
  if [[ "$STORAGE" == s3 ]]; then
    files+=(-f "$tree/infra/rust/compose.s3.yml")
    [[ "$project" == "$ROLLBACK_PROJECT" && -f "$RB_BUCKET_OVERLAY" ]] && files+=(-f "$RB_BUCKET_OVERLAY")
  fi
  docker compose "${files[@]}" --project-name "$project" --env-file "$env_file" "$@"
}

# Containers, volumes and networks that still carry the project's compose label.
owned_resources() {
  local filter="label=com.docker.compose.project=$1" containers volumes networks
  containers="$(docker ps -a -q --filter "$filter")" || return 1
  volumes="$(docker volume ls -q --filter "$filter")" || return 1
  networks="$(docker network ls -q --filter "$filter")" || return 1
  printf '%s\n' "$containers" "$volumes" "$networks" | awk 'NF' | paste -sd' ' -
}

# `down -v` with the tree the project was started from, then prove nothing is left.
teardown_project() {
  local project="$1" env_file="$2" tree="$3" left rc=0
  if [[ -f "$env_file" ]]; then
    project_compose "$project" "$env_file" "$tree" down -v --remove-orphans 2>&1 \
      | redact >"$EVIDENCE_DIR/teardown-${project}.log"
    if (( PIPESTATUS[0] != 0 )); then
      echo "cleanup: down failed for $project (see $EVIDENCE_DIR/teardown-${project}.log)" >&2
      rc=1
    fi
  fi
  if ! left="$(owned_resources "$project")"; then
    echo "cleanup: could not list the resources of $project" >&2
    return 1
  fi
  if [[ -n "$left" ]]; then
    echo "cleanup: $project still has: $left" >&2
    rc=1
  fi
  return "$rc"
}

cleanup() {
  local status=$?
  set +e
  if (( status == 0 )); then
    local torn=0
    if (( STACK_STARTED )); then
      # Rollback first: with S3 its server joins the upgrade project's network.
      teardown_project "$ROLLBACK_PROJECT" "$ROLLBACK_ENV" "$OLD_TREE" || torn=1
      teardown_project "$UPGRADE_PROJECT" "$UPGRADE_ENV" "$UPGRADE_TREE" || torn=1
    fi
    if (( torn )); then
      printf 'FAIL: cleanup left resources or failed\n' >>"$ASSERT_LOG"
      echo "upgrade-smoke passed its checks but cleanup failed; kept for diagnosis:" >&2
      echo "  projects: $UPGRADE_PROJECT $ROLLBACK_PROJECT (docker compose -p NAME down -v)" >&2
      echo "  work dir (0700, env files hold generated secrets): $WORK" >&2
      status=1
    else
      (( STACK_STARTED )) && log_assert "cleanup: both projects down, no container, volume or network left: ok"
      rm -rf "$WORK"
    fi
  elif (( STACK_STARTED == 0 )); then
    # Nothing was started and no env file was written; the work dir holds only
    # the two source archives and recipes.
    rm -rf "$WORK"
  else
    [[ -f "$UPGRADE_ENV" ]] && project_compose "$UPGRADE_PROJECT" "$UPGRADE_ENV" "$UPGRADE_TREE" logs --no-color --tail 200 \
      2>&1 | redact >"$EVIDENCE_DIR/failure-${UPGRADE_PROJECT}.log"
    [[ -f "$ROLLBACK_ENV" ]] && project_compose "$ROLLBACK_PROJECT" "$ROLLBACK_ENV" "$OLD_TREE" logs --no-color --tail 200 \
      2>&1 | redact >"$EVIDENCE_DIR/failure-${ROLLBACK_PROJECT}.log"
    echo "upgrade-smoke failed after $((SECONDS - START_TS))s; kept for diagnosis:" >&2
    echo "  projects: $UPGRADE_PROJECT $ROLLBACK_PROJECT (docker compose -p NAME down -v)" >&2
    echo "  work dir (0700, env files hold generated secrets): $WORK" >&2
  fi
  echo "evidence (redacted, kept): $EVIDENCE_DIR" >&2
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

[[ -f "$FIXTURE_HWPX" ]] || fail "missing HWPX fixture: $FIXTURE_HWPX"

# --- Source identity -------------------------------------------------------
OLD_SHA="$(git -C "$ROOT" rev-parse --verify "${OLD_REF}^{commit}")"
NEW_SHA="$(git -C "$ROOT" rev-parse --verify "${NEW_REF}^{commit}")"
MAIN_SHA="$(git -C "$ROOT" rev-parse --verify "${MAIN_REF}^{commit}")"
[[ "$OLD_SHA" != "$NEW_SHA" ]] || fail "old and new SHA are the same commit"
git -C "$ROOT" merge-base --is-ancestor "$OLD_SHA" "$NEW_SHA" || fail "old $OLD_SHA is not an ancestor of new $NEW_SHA"
FIRST_PARENT="$(git -C "$ROOT" rev-list --first-parent "$MAIN_SHA")"
for sha in "$OLD_SHA" "$NEW_SHA"; do
  grep -qx "$sha" <<<"$FIRST_PARENT" || fail "$sha is not on the first-parent history of $MAIN_REF ($MAIN_SHA)"
  log_assert "source ${sha}: on first-parent ${MAIN_REF}@${MAIN_SHA}: $(git -C "$ROOT" log -1 --format='%cI %s' "$sha")"
done

migration_versions() {
  git -C "$ROOT" ls-tree --name-only "$1" migrations/ \
    | sed -nE 's#^migrations/0*([0-9]+)_.*\.sql$#\1#p' | sort -n | paste -sd, -
}
OLD_VERSIONS="$(migration_versions "$OLD_SHA")"
NEW_VERSIONS="$(migration_versions "$NEW_SHA")"
version_diff() {
  comm "$1" <(tr , '\n' <<<"$2" | sort) <(tr , '\n' <<<"$3" | sort) | sort -n | paste -sd, -
}
# A later main commit may add a migration numbered below the old maximum
# (applied out of order), so compare sets rather than prefixes.
[[ -z "$(version_diff -23 "$OLD_VERSIONS" "$NEW_VERSIONS")" ]] || fail "new drops old migrations: old=$OLD_VERSIONS new=$NEW_VERSIONS"
ADDED="$(version_diff -13 "$OLD_VERSIONS" "$NEW_VERSIONS")"
LAST_VERSION="${NEW_VERSIONS##*,}"
[[ "$ADDED" == *,* && ",${ADDED}," == *",${LAST_VERSION},"* ]] \
  || fail "new must add at least two migrations including its newest ($ADDED); a partial upgrade is not observable"
PARTIAL_VERSIONS="${NEW_VERSIONS%,*}"
LAST_FILE="$(git -C "$ROOT" ls-tree --name-only "$NEW_SHA" migrations/ | grep -E "^migrations/0*${LAST_VERSION}_")"
BLOCKER_TABLE="$(git -C "$ROOT" show "$NEW_SHA:$LAST_FILE" | sed -nE 's/^CREATE TABLE (fvoci\.[a-z_]+) \(.*/\1/p' | head -1)"
[[ -n "$BLOCKER_TABLE" ]] || fail "no CREATE TABLE fvoci.* in $LAST_FILE to block"
log_assert "migrations: old=${OLD_VERSIONS##*,} new=${LAST_VERSION} added=${ADDED}; injected blocker ${BLOCKER_TABLE} (${LAST_FILE})"

export_tree() {
  local sha="$1" dest="$2"
  mkdir -p "$dest"
  git -C "$ROOT" archive --format=tar "$sha" | tar -x -C "$dest"
}
export_tree "$OLD_SHA" "$OLD_TREE"
export_tree "$NEW_SHA" "$NEW_TREE"
if [[ "$STORAGE" == s3 ]]; then
  for tree in "$OLD_TREE" "$NEW_TREE"; do
    [[ -f "$tree/infra/rust/compose.s3.yml" ]] || fail "no infra/rust/compose.s3.yml in ${tree##*/}"
  done
  # The old checkout's backup.sh must refuse S3; the smoke checks that it does.
  grep -q 'STORAGE_DRIVER=s3: this script archives the local storage volume' "$OLD_TREE/scripts/backup.sh" \
    || fail "old backup.sh has no S3 refusal"
  log_assert "storage s3: compose.s3.yml in both trees, old backup.sh refuses S3"
fi

# --- Images ----------------------------------------------------------------
docker_free_gib() {
  df -P -BG "$(docker info -f '{{.DockerRootDir}}' 2>/dev/null || echo /)" 2>/dev/null \
    | awk 'NR==2 { sub(/G$/, "", $4); print $4 }'
}

# Prints the image ID. The derived recipe differs from the archived Dockerfile
# by exactly one line: ENV CARGO_BUILD_JOBS in the Rust build stage.
ensure_image() {
  local sha="$1" tree="$2" name="$3"
  local tag="fvoci-rust-install:upgrade-${sha:0:12}"
  local src="$tree/infra/rust/Dockerfile" recipe="$WORK/Dockerfile.${name}"
  awk -v jobs="$BUILD_JOBS" '{ print } /^FROM rust-deps AS rust-build$/ { print "ENV CARGO_BUILD_JOBS=" jobs; n++ } END { exit n == 1 ? 0 : 1 }' \
    "$src" >"$recipe" || fail "${name}: Rust build stage not found exactly once in $src"
  local delta
  delta="$(diff "$src" "$recipe" | grep -E '^[<>]' || true)"
  [[ "$delta" == "> ENV CARGO_BUILD_JOBS=${BUILD_JOBS}" ]] || fail "${name}: recipe delta is not the single jobs line: $delta"
  local src_hash recipe_hash
  src_hash="$(sha256sum "$src" | awk '{print $1}')"
  recipe_hash="$(sha256sum "$recipe" | awk '{print $1}')"
  local labels
  labels="$(docker image inspect -f '{{index .Config.Labels "org.opencontainers.image.revision"}} {{index .Config.Labels "io.fvoci.upgrade-smoke.recipe-sha256"}}' "$tag" 2>/dev/null || true)"
  if [[ "$labels" == "$sha $recipe_hash" ]]; then
    log_assert "${name} image ${tag}: reused (labels match source and recipe)" >&2
  else
    [[ -z "$labels" ]] || fail "${name}: tag $tag exists with other labels ($labels); not reusing or overwriting it"
    local free
    free="$(docker_free_gib)"
    if [[ -z "$free" ]] || (( free < MIN_FREE_GIB )); then
      fail "${name}: ${free:-?} GiB free under the Docker root, need ${MIN_FREE_GIB}; not building"
    fi
    if (( PLAN_ONLY )); then
      log_assert "${name} image ${tag}: would build, disk gate ok (${free} GiB free, need ${MIN_FREE_GIB}); recipe-sha256=${recipe_hash}" >&2
      return 0
    fi
    log_assert "== build ${name} image ${tag} from git archive ${sha} (jobs ${BUILD_JOBS}, ${free} GiB free)" >&2
    local started=$SECONDS
    if ! docker build -f "$recipe" \
      --label "org.opencontainers.image.revision=${sha}" \
      --label "io.fvoci.upgrade-smoke.dockerfile-sha256=${src_hash}" \
      --label "io.fvoci.upgrade-smoke.recipe-sha256=${recipe_hash}" \
      -t "$tag" "$tree" >"$EVIDENCE_DIR/build-${name}.log" 2>&1; then
      tail -40 "$EVIDENCE_DIR/build-${name}.log" >&2
      fail "${name}: docker build failed (log: $EVIDENCE_DIR/build-${name}.log)"
    fi
    log_assert "${name} image built: ok ($((SECONDS - started))s, $(docker_free_gib) GiB free after)" >&2
  fi
  log_assert "${name} image: tag=${tag} dockerfile-sha256=${src_hash} recipe-sha256=${recipe_hash}" >&2
  docker image inspect -f '{{.Id}}' "$tag"
}
OLD_IMAGE_ID="$(ensure_image "$OLD_SHA" "$OLD_TREE" old)"
NEW_IMAGE_ID="$(ensure_image "$NEW_SHA" "$NEW_TREE" new)"
if (( PLAN_ONLY )); then
  log_assert "== plan only: nothing built, started or torn down; the disk gate ran only for images that need a build"
  exit 0
fi
OLD_TAG="fvoci-rust-install:upgrade-${OLD_SHA:0:12}"
NEW_TAG="fvoci-rust-install:upgrade-${NEW_SHA:0:12}"
[[ "$OLD_IMAGE_ID" != "$NEW_IMAGE_ID" ]] || fail "old and new tags resolve to the same image $OLD_IMAGE_ID"
log_assert "images distinct: old=${OLD_IMAGE_ID} new=${NEW_IMAGE_ID}"

# --- Helpers ---------------------------------------------------------------
pick_port() {
  python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'
}

wait_http() {
  local base="$1" deadline=$((SECONDS + 60))
  while (( SECONDS < deadline )); do
    curl -fsS "$base/api/v1/setup" >/dev/null 2>&1 && return 0
    sleep 0.5
  done
  fail "timed out waiting for $base/api/v1/setup"
}

sql() {
  local project="$1" env_file="$2" tree="$3" query="$4"
  project_compose "$project" "$env_file" "$tree" exec -T postgres \
    psql -X -v ON_ERROR_STOP=1 -U fvoci_owner -d fvoci -tAc "$query" | tr -d '[:space:]'
}

applied_versions() {
  sql "$1" "$2" "$3" "SELECT string_agg(version::text, ',' ORDER BY version) FROM fvoci.schema_migrations"
}

service_ids() {
  docker ps -a -q --filter "label=com.docker.compose.project=$1" --filter "label=com.docker.compose.service=$2"
}

running_ids() {
  docker ps -q --filter "label=com.docker.compose.project=$1" --filter "label=com.docker.compose.service=$2"
}

# One container of the service, running, from the expected image.
expect_service_image() {
  local project="$1" service="$2" image="$3" ids
  ids="$(running_ids "$project" "$service")"
  [[ -n "$ids" && "$(wc -l <<<"$ids")" == 1 ]] || fail "$project/$service: expected one running container, got '${ids}'"
  [[ "$(docker inspect -f '{{.Image}}' "$ids")" == "$image" ]] || fail "$project/$service does not run image $image"
}

login() {
  local base="$1"
  : >"$COOKIE_JAR"
  curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $base" \
    -X POST "$base/api/v1/auth/login" -d "{\"email\":\"${OWNER_EMAIL}\",\"password\":\"${OWNER_LOGIN_PASSWORD}\"}" \
    | python3 -c 'import json,sys; assert json.load(sys.stdin).get("userId")'
}

check_seeded_data() {
  local base="$1" label="$2"
  curl -fsS -b "$COOKIE_JAR" "$base/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/body" \
    | python3 -c 'import json,sys; body=json.load(sys.stdin); assert body["contentJson"]==json.loads(sys.argv[1])["contentJson"], body' "$BODY_BEFORE" \
    || fail "$label: document body differs"
  local id
  for id in "${ATTACHMENT_IDS[@]}"; do
    curl -fsS -b "$COOKIE_JAR" "$base/api/v1/workspaces/${WORKSPACE_ID}/attachments/${id}/download" -o "$DOWNLOAD_PATH"
    [[ "$(sha256sum "$DOWNLOAD_PATH" | awk '{print $1}')" == "$FIXTURE_SHA" ]] || fail "$label: attachment $id bytes differ"
  done
  curl -fsS -b "$COOKIE_JAR" "$base/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/comments" \
    | python3 -c '
import json, sys
body = json.load(sys.stdin)
items = body.get("items", body if isinstance(body, list) else [])
assert any(c.get("id") == sys.argv[1] and c.get("body") == "업그레이드 댓글 🙂" for c in items), body
' "$COMMENT_ID" || fail "$label: comment missing"
  log_assert "${label}: login, document body, ${#ATTACHMENT_IDS[@]} attachment(s) sha256 ${FIXTURE_SHA:0:12}, comment: ok"
}

write_env() {
  local dest="$1" image="$2" port="$3"
  cat >"$dest" <<EOF
FVOCI_IMAGE=${image}
POSTGRES_DB=fvoci
POSTGRES_USER=fvoci_owner
POSTGRES_PASSWORD=${OWNER_PASSWORD}
FVOCI_APP_ROLE=fvoci_app
FVOCI_APP_PASSWORD=${APP_PASSWORD}
PASSWORD_PEPPER_KEYS=${PEPPER}
PASSWORD_PEPPER_ACTIVE_KEY_ID=install
ENCRYPTION_KEYS=${ENCRYPTION_KEYS}
ENCRYPTION_ACTIVE_KEY_ID=k1
FVOCI_PUBLIC_ORIGIN=http://127.0.0.1:${port}
FVOCI_COOKIE_SECURE=false
FVOCI_PUBLISH_PORT=${port}
FVOCI_EXTRACT_POLL_SECS=2
MEILI_MASTER_KEY=${MEILI_MASTER_KEY}
EOF
  if [[ "$STORAGE" == s3 ]]; then
    # compose.s3.yml's silo uses these as its root credentials; run-owned.
    printf 'S3_BUCKET=%s\nS3_ACCESS_KEY_ID=%s\nS3_SECRET_ACCESS_KEY=%s\n' \
      "$S3_BUCKET_NAME" "$S3_ACCESS_KEY" "$S3_SECRET_KEY" >>"$dest"
  fi
  chmod 600 "$dest"
}

OWNER_PASSWORD="$(openssl rand -hex 16)"
APP_PASSWORD="$(openssl rand -hex 16)"
MEILI_MASTER_KEY="$(openssl rand -hex 16)"
PEPPER_KEY="$(openssl rand -hex 32)"
PEPPER="{\"install\":\"${PEPPER_KEY}\"}"
ENC_K1="$(openssl rand -hex 32)"
ENCRYPTION_KEYS="{\"k1\":\"${ENC_K1}\"}"
OWNER_EMAIL="owner@upgrade.test"
OWNER_LOGIN_PASSWORD="upgradepass1"
S3_BUCKET_NAME="fvoci-up-${RUN_ID}"
S3_ACCESS_KEY="fvoci$(openssl rand -hex 8)"
S3_SECRET_KEY="$(openssl rand -hex 24)"
SECRETS=("$OWNER_PASSWORD" "$APP_PASSWORD" "$MEILI_MASTER_KEY" "$PEPPER_KEY" "$ENC_K1" "$OWNER_LOGIN_PASSWORD"
  "$S3_ACCESS_KEY" "$S3_SECRET_KEY")
FIXTURE_SHA="$(sha256sum "$FIXTURE_HWPX" | awk '{print $1}')"

UP=("$UPGRADE_PROJECT" "$UPGRADE_ENV")

# mcli inside the upgrade project's silo, against the bucket every project in
# this run uses. The credentials stay in that container's environment
# (MC_HOST_b), never in host argv.
bucket_mc() {
  # shellcheck disable=SC2016 # expanded by the container's shell
  project_compose "${UP[@]}" "$UPGRADE_TREE" exec -T silo sh -c \
    'MC_HOST_b="http://${MINIO_ROOT_USER}:${MINIO_ROOT_PASSWORD}@127.0.0.1:9000" exec mcli --json "$@"' mcli "$@"
}

storage_key() {
  sql "${UP[@]}" "$UPGRADE_TREE" "SELECT storage_key FROM fvoci.attachments WHERE id='$1' AND status='stored'"
}

# Every version of KEY as `<versionId> <size> <deleteMarker> <latest>` lines.
object_versions() {
  bucket_mc ls --versions "b/${S3_BUCKET_NAME}/$1" | python3 -c '
import json, sys
for line in sys.stdin:
    if line.strip():
        v = json.loads(line)
        assert v.get("status") == "success", v
        print(v["versionId"], v.get("size", 0), str(bool(v.get("isDeleteMarker"))).lower(), str(bool(v.get("isLatest"))).lower())
'
}

# fvoci-migrate --verify-storage with the server's environment of PROJECT; sets
# VS_STATUS and VS_OUT (redacted copy in the evidence dir).
verify_storage() {
  local label="$1" project="$2" env_file="$3" tree="$4"
  shift 4
  if VS_OUT="$(project_compose "$project" "$env_file" "$tree" run --rm --no-deps "$@" \
    --entrypoint /opt/fvoci/bin/fvoci-migrate server --verify-storage 2>&1)"; then
    VS_STATUS=0
  else
    VS_STATUS=$?
  fi
  redact <<<"$VS_OUT" >"$EVIDENCE_DIR/verify-storage-${label}.log"
}

# Asserts the single JSON report of the last verify_storage call.
expect_storage_report() {
  python3 -c '
import json, sys
reports = [l for l in sys.stdin.read().splitlines() if l.startswith("{")]
assert len(reports) == 1, reports
r = json.loads(reports[0])
want = {"checked": int(sys.argv[1]), "missing": sorted(filter(None, sys.argv[2].split(","))),
        "sizeMismatch": sorted(filter(None, sys.argv[3].split(",")))}
got = {"checked": r["checked"], "missing": sorted(r["missing"]), "sizeMismatch": sorted(r["sizeMismatch"])}
assert got == want and not r["previewMissing"] and not r["previewSizeMismatch"], (got, want, r)
' "$@" <<<"$VS_OUT"
}

# --- 1. Old install with data ---------------------------------------------
PORT="$(pick_port)"
BASE="http://127.0.0.1:${PORT}"
write_env "$UPGRADE_ENV" "$OLD_TAG" "$PORT"
log_assert "== old install ${UPGRADE_PROJECT} on ${OLD_TAG}"
STACK_STARTED=1
project_compose "${UP[@]}" "$OLD_TREE" up -d --wait server
expect_service_image "$UPGRADE_PROJECT" server "$OLD_IMAGE_ID"
[[ "$(applied_versions "${UP[@]}" "$OLD_TREE")" == "$OLD_VERSIONS" ]] || fail "old install schema is not $OLD_VERSIONS"
wait_http "$BASE"
log_assert "old server up on old image, schema through ${OLD_VERSIONS##*,}: ok"

curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $BASE" \
  -X POST "$BASE/api/v1/setup" \
  -d "{\"email\":\"${OWNER_EMAIL}\",\"password\":\"${OWNER_LOGIN_PASSWORD}\",\"givenName\":\"Owner\",\"workspaceSlug\":\"upgrade\",\"workspaceName\":\"Upgrade\"}" >/dev/null
SESSION="$(awk '$6 == "fvoci_session" { print $7; exit }' "$COOKIE_JAR")"
[[ -n "$SESSION" ]] || fail "setup did not return a session cookie"
login "$BASE"
SESSION="$(awk '$6 == "fvoci_session" { print $7; exit }' "$COOKIE_JAR")"
SECRETS+=("$SESSION")
WORKSPACE_ID="$(curl -fsS -b "$COOKIE_JAR" "$BASE/api/v1/me/workspaces" | python3 -c 'import json,sys; print(json.load(sys.stdin)["items"][0]["id"])')"
DOCUMENT_ID="$(curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $BASE" \
  -X POST "$BASE/api/v1/workspaces/${WORKSPACE_ID}/documents" -d '{"parentId":null,"title":"Upgrade doc"}' \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
if [[ "$STORAGE" == s3 ]]; then
  # RUNNING.md "S3 storage backup", item 1: the operator enables versioning
  # before relying on it. The server wrote no object yet (fresh install).
  bucket_mc version enable "b/${S3_BUCKET_NAME}" >/dev/null
  bucket_mc version info "b/${S3_BUCKET_NAME}" | python3 -c '
import json, sys
info = json.load(sys.stdin)
assert info.get("versioning", {}).get("status") == "Enabled", info
' || fail "bucket versioning is not enabled"
  [[ "$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$(running_ids "$UPGRADE_PROJECT" server)" \
    | grep -c '^STORAGE_DRIVER=s3$')" == 1 ]] || fail "old server does not run with STORAGE_DRIVER=s3"
  log_assert "old server on STORAGE_DRIVER=s3, run-owned silo bucket ${S3_BUCKET_NAME} versioning Enabled: ok"
fi
# The old checkout's own collab client, matched to the old server's API.
BODY_BEFORE="$(node "$OLD_TREE/scripts/install-smoke-collab.mjs" --base-url "$BASE" --origin "$BASE" \
  --session "$SESSION" --workspace-id "$WORKSPACE_ID" --document-id "$DOCUMENT_ID")"
grep -q '"contentJson"' <<<"$BODY_BEFORE" || fail "collab body projection failed on old image"

# Uploads the HWPX fixture under NAME and waits for extraction `ok`; prints the id.
upload_fixture() {
  local name="$1" init id part_url etag status=pending
  init="$(curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $BASE" \
    -X POST "$BASE/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/uploads" \
    -d "{\"name\":\"${name}\",\"sizeBytes\":$(wc -c <"$FIXTURE_HWPX"),\"declaredMime\":\"application/x-hwp\"}")"
  id="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["attachmentId"])' "$init")"
  part_url="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["parts"][0]["url"])' "$init")"
  etag="$(curl -fsS -b "$COOKIE_JAR" -H "origin: $BASE" -X PUT "$BASE${part_url}" \
    --data-binary @"$FIXTURE_HWPX" -D - -o /dev/null | awk '/^[Ee]tag:/ { print $2; exit }' | tr -d '\r')"
  curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $BASE" \
    -X POST "$BASE/api/v1/workspaces/${WORKSPACE_ID}/attachments/${id}/complete" \
    -d "{\"parts\":[{\"partNumber\":1,\"etag\":\"${etag}\"}]}" >/dev/null
  for _ in $(seq 1 90); do
    status="$(sql "${UP[@]}" "$OLD_TREE" "SELECT extract_status FROM fvoci.attachments WHERE id='${id}'")"
    [[ "$status" == pending ]] || break
    sleep 1
  done
  [[ "$status" == ok ]] || fail "old image extraction status of ${name}: $status"
  printf '%s\n' "$id"
}
ATTACHMENT_ID="$(upload_fixture sample.hwpx)"
ATTACHMENT_IDS=("$ATTACHMENT_ID")
if [[ "$STORAGE" == s3 ]]; then
  # A second stored object, so one can be deleted and the other overwritten.
  ATTACHMENT_B_ID="$(upload_fixture sample-b.hwpx)"
  ATTACHMENT_IDS+=("$ATTACHMENT_B_ID")
fi
COMMENT_ID="$(curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $BASE" \
  -X POST "$BASE/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/comments" \
  -d '{"body":"업그레이드 댓글 🙂"}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
# A TOTP secret sealed with ENCRYPTION_KEYS k1 (setup only; login stays single-factor).
curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $BASE" \
  -X POST "$BASE/api/v1/auth/mfa/setup" -d "{\"currentPassword\":\"${OWNER_LOGIN_PASSWORD}\"}" >/dev/null
SEALED_SQL="SELECT count(*) FROM fvoci.user_mfa WHERE totp_secret LIKE 'enc:v2:k1:%'"
[[ "$(sql "${UP[@]}" "$OLD_TREE" "$SEALED_SQL")" == 1 ]] || fail "expected one sealed MFA secret on old image"
check_seeded_data "$BASE" "old image seeded"
log_assert "old image seed: collab body, HWPX extract ok, sealed MFA secret (k1): ok"

# --- 2. Pre-upgrade backup with the old checkout ---------------------------
if [[ "$STORAGE" == local ]]; then
  log_assert "== old checkout backup.sh --leave-stopped"
  bash "$OLD_TREE/scripts/backup.sh" --project "$UPGRADE_PROJECT" --env-file "$UPGRADE_ENV" \
    --output "$BACKUP_DIR" --leave-stopped 2>&1 | redact >"$EVIDENCE_DIR/backup.log"
else
  # Negative control: the volume archive must refuse S3 without stopping anything.
  if bash "$OLD_TREE/scripts/backup.sh" --project "$UPGRADE_PROJECT" --env-file "$UPGRADE_ENV" \
    --output "$WORK/refused-backup" --leave-stopped >"$WORK/refused-backup.log" 2>&1; then
    fail "old backup.sh accepted an S3 install"
  fi
  redact <"$WORK/refused-backup.log" >"$EVIDENCE_DIR/backup-s3-refused.log"
  grep -q 'STORAGE_DRIVER=s3' "$EVIDENCE_DIR/backup-s3-refused.log" || fail "backup.sh failed for another reason"
  [[ ! -e "$WORK/refused-backup" && -n "$(running_ids "$UPGRADE_PROJECT" server)" ]] \
    || fail "refused backup.sh left output or stopped the server"
  log_assert "old backup.sh refuses STORAGE_DRIVER=s3, no output, server still running: ok"

  # RUNNING.md "S3 storage backup", item 2 and "Upgrade" step 2 for S3: the
  # documented stop, then the same quiesced pg_dump backup.sh takes.
  log_assert "== documented server stop, quiesced pg_dump (S3)"
  OLD_CID="$(running_ids "$UPGRADE_PROJECT" server)"
  project_compose "${UP[@]}" "$OLD_TREE" stop -t 45 server
  [[ "$(docker inspect -f '{{.State.Status}} {{.State.ExitCode}} {{.State.OOMKilled}}' "$OLD_CID")" == "exited 0 false" ]] \
    || fail "old server did not stop cleanly"
  [[ "$(sql "${UP[@]}" "$OLD_TREE" "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND backend_type='client backend'")" == 0 ]] \
    || fail "other database sessions remain after stopping the server"
  mkdir -m 700 "$BACKUP_DIR"
  # shellcheck disable=SC2016 # expanded in the postgres container, as in backup.sh
  project_compose "${UP[@]}" "$OLD_TREE" exec -T postgres sh -c \
    'pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" --format=custom --no-owner --schema=public --schema=fvoci' \
    >"$BACKUP_DIR/database.dump"
  chmod 600 "$BACKUP_DIR/database.dump"
  [[ "$(head -c 5 "$BACKUP_DIR/database.dump")" == PGDMP ]] || fail "pg_dump output is not custom format"
  # Taken after the dump, whole seconds, like backup.sh's manifest createdAt.
  DUMP_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  KEY_A="$(storage_key "$ATTACHMENT_ID")"
  KEY_B="$(storage_key "$ATTACHMENT_B_ID")"
  [[ -n "$KEY_A" && -n "$KEY_B" && "$KEY_A" != "$KEY_B" ]] || fail "stored keys not found"
  for key in "$KEY_A" "$KEY_B"; do
    object_versions "$key" >"$WORK/versions-before"
    if [[ "$(wc -l <"$WORK/versions-before")" != 1 ]] || ! grep -q " $(wc -c <"$FIXTURE_HWPX") false true$" "$WORK/versions-before"; then
      fail "expected one live version of $key before the upgrade: $(cat "$WORK/versions-before")"
    fi
  done
  log_assert "pg_dump $(wc -c <"$BACKUP_DIR/database.dump") bytes at ${DUMP_AT}, 0 other sessions; 2 stored objects, one live version each: ok"
fi
cp -p "$UPGRADE_ENV" "$BACKUP_ENV"
[[ -z "$(running_ids "$UPGRADE_PROJECT" server)" ]] || fail "server still running after the pre-upgrade backup"
log_assert "pre-upgrade backup taken, old server stopped, env copy kept (0600): ok"

# --- 3. Upgrade whose init fails -------------------------------------------
sed -i "s#^FVOCI_IMAGE=.*#FVOCI_IMAGE=${NEW_TAG}#" "$UPGRADE_ENV"
UPGRADE_TREE="$NEW_TREE"
sql "${UP[@]}" "$NEW_TREE" "CREATE TABLE ${BLOCKER_TABLE} (blocker integer)" >/dev/null
log_assert "== upgrade attempt 1 with ${BLOCKER_TABLE} pre-created (must fail)"
set +e
project_compose "${UP[@]}" "$NEW_TREE" up -d --wait server >"$WORK/attempt1.log" 2>&1
ATTEMPT1=$?
set -e
redact <"$WORK/attempt1.log" >"$EVIDENCE_DIR/upgrade-attempt1.log"
(( ATTEMPT1 != 0 )) || fail "upgrade with a blocked migration succeeded"
INIT_IDS="$(service_ids "$UPGRADE_PROJECT" init)"
[[ -n "$INIT_IDS" && "$(wc -l <<<"$INIT_IDS")" == 1 ]] || fail "expected one init container, got '${INIT_IDS}'"
read -r INIT_IMAGE INIT_STATE INIT_EXIT < <(docker inspect -f '{{.Image}} {{.State.Status}} {{.State.ExitCode}}' "$INIT_IDS")
[[ "$INIT_IMAGE" == "$NEW_IMAGE_ID" && "$INIT_STATE" == exited && "$INIT_EXIT" != 0 ]] \
  || fail "init expected new image exited nonzero, got ${INIT_IMAGE} ${INIT_STATE} ${INIT_EXIT}"
docker logs "$INIT_IDS" 2>&1 | redact >"$EVIDENCE_DIR/init-failure.log"
grep -q "${BLOCKER_TABLE#fvoci.}" "$EVIDENCE_DIR/init-failure.log" || fail "init log does not name the blocked table"
[[ -z "$(running_ids "$UPGRADE_PROJECT" server)" ]] || fail "a server is running after failed init"
for id in $(service_ids "$UPGRADE_PROJECT" server); do
  read -r image started < <(docker inspect -f '{{.Image}} {{.State.StartedAt}}' "$id")
  # A recreated new-image server must never have started; an old one stays stopped.
  if [[ "$image" == "$NEW_IMAGE_ID" && "$started" != 0001-01-01T00:00:00Z ]]; then
    fail "new-image server container $id started despite failed init"
  fi
done
if curl -fsS -m 2 "$BASE/api/v1/setup" >/dev/null 2>&1; then
  fail "HTTP answered on $BASE after failed init"
fi
PARTIAL="$(applied_versions "${UP[@]}" "$NEW_TREE")"
[[ "$PARTIAL" == "$PARTIAL_VERSIONS" ]] || fail "after failed init expected schema $PARTIAL_VERSIONS, got $PARTIAL"
log_assert "failed init: exit ${INIT_EXIT} on new image, server stopped, migrations through ${PARTIAL##*,} committed and ${LAST_VERSION} not: ok"

# --- 4. Fix the cause, one retry ------------------------------------------
sql "${UP[@]}" "$NEW_TREE" "DROP TABLE ${BLOCKER_TABLE}" >/dev/null
log_assert "== upgrade attempt 2 after removing the blocker (single retry)"
project_compose "${UP[@]}" "$NEW_TREE" up -d --wait server 2>&1 | redact >"$EVIDENCE_DIR/upgrade-attempt2.log"
INIT_IDS="$(service_ids "$UPGRADE_PROJECT" init)"
[[ "$(docker inspect -f '{{.Image}} {{.State.ExitCode}}' "$INIT_IDS")" == "$NEW_IMAGE_ID 0" ]] || fail "retry init did not exit 0 on the new image"
expect_service_image "$UPGRADE_PROJECT" server "$NEW_IMAGE_ID"
for id in $(docker ps -a -q --filter "label=com.docker.compose.project=$UPGRADE_PROJECT"); do
  [[ "$(docker inspect -f '{{.Image}}' "$id")" != "$OLD_IMAGE_ID" ]] || fail "old-image container $id remains on the migrated project"
done
[[ "$(applied_versions "${UP[@]}" "$NEW_TREE")" == "$NEW_VERSIONS" ]] || fail "upgraded schema is not $NEW_VERSIONS"
wait_http "$BASE"
log_assert "retry: init exit 0, server on new image, schema through ${LAST_VERSION}, no old-image container in project: ok"

# --- 5. Upgraded data -------------------------------------------------------
DOCTOR="$(project_compose "${UP[@]}" "$NEW_TREE" exec -T server /opt/fvoci/bin/fvoci-migrate --doctor)" \
  || { redact <<<"$DOCTOR" >&2; fail "doctor failed on upgraded install"; }
python3 -c 'import json,sys; assert json.load(sys.stdin)["ok"] is True' <<<"$DOCTOR" || fail "doctor not ok"
login "$BASE"
check_seeded_data "$BASE" "upgraded"
[[ "$(sql "${UP[@]}" "$NEW_TREE" "SELECT extract_status FROM fvoci.attachments WHERE id='${ATTACHMENT_ID}'")" == ok ]] \
  || fail "upgraded extraction status lost"
[[ "$(sql "${UP[@]}" "$NEW_TREE" "$SEALED_SQL")" == 1 ]] || fail "sealed MFA secret changed after upgrade"
VERIFY="$(project_compose "${UP[@]}" "$NEW_TREE" run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate server --verify-secrets 2>&1)" \
  || { redact <<<"$VERIFY" >&2; fail "verify-secrets failed after upgrade"; }
redact <<<"$VERIFY" >"$EVIDENCE_DIR/verify-secrets-upgraded.log"
# Negative control: the same probe with a different k1 must fail to open the
# sealed MFA secret (AEAD failure: `invalid`, not a missing key id or a DB error).
WRONG_K1="$(openssl rand -hex 32)"
SECRETS+=("$WRONG_K1")
if WRONG_VERIFY="$(project_compose "${UP[@]}" "$NEW_TREE" run --rm --no-deps -e "ENCRYPTION_KEYS={\"k1\":\"${WRONG_K1}\"}" \
  --entrypoint /opt/fvoci/bin/fvoci-migrate server --verify-secrets 2>&1)"; then
  WRONG_STATUS=0
else
  WRONG_STATUS=$?
fi
redact <<<"$WRONG_VERIFY" >"$EVIDENCE_DIR/verify-secrets-wrong-key.log"
(( WRONG_STATUS != 0 )) || fail "verify-secrets accepted a different k1"
python3 -c '
import json, sys
reports = [line for line in sys.stdin.read().splitlines() if line.startswith("{")]
assert len(reports) == 1, reports
mfa = json.loads(reports[0])["userMfa"]
assert mfa["checked"] == 1 and len(mfa["invalid"]) == 1 and mfa["keyUnavailable"] == [], mfa
' <"$EVIDENCE_DIR/verify-secrets-wrong-key.log" || fail "wrong-key verify-secrets did not report the MFA secret invalid"
grep -q 'do not open with the configured ENCRYPTION_KEYS' "$EVIDENCE_DIR/verify-secrets-wrong-key.log" \
  || fail "wrong-key verify-secrets failed for another reason (exit ${WRONG_STATUS})"
POST_DOC="$(curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $BASE" \
  -X POST "$BASE/api/v1/workspaces/${WORKSPACE_ID}/documents" -d '{"parentId":null,"title":"After upgrade"}' \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
if [[ "$STORAGE" == s3 ]]; then
  verify_storage upgraded "${UP[@]}" "$NEW_TREE"
  (( VS_STATUS == 0 )) || fail "verify-storage failed on the upgraded install"
  expect_storage_report 2 "" "" || fail "unexpected verify-storage report on the upgraded install"
  log_assert "upgraded: new image --verify-storage checked 2, none missing or mismatched: ok"
fi
log_assert "upgraded: doctor ok, extraction kept, sealed secret opens with k1 and is invalid under another k1 (exit ${WRONG_STATUS}), new write ok: ok"

log_assert "== stop upgraded server before rollback"
UPGRADED_CID="$(running_ids "$UPGRADE_PROJECT" server)"
project_compose "${UP[@]}" "$NEW_TREE" stop -t 45 server
[[ "$(docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' "$UPGRADED_CID")" == "exited 0" ]] || fail "upgraded server did not stop cleanly"

# The old restore.sh sequence without the volume archive (RUNNING.md "S3 storage
# backup", item 3): fresh PostgreSQL, app role, pg_restore, init, outbox
# rebase and search rebuild, all before any server of the project starts.
# shellcheck disable=SC2016 # sh -c bodies expand in the postgres container, as in restore.sh
s3_restore_db() {
  local pg_cid snapshot_at since
  project_compose "${RB[@]}" "$OLD_TREE" up -d --wait postgres meilisearch || return 1
  pg_cid="$(project_compose "${RB[@]}" "$OLD_TREE" ps -q postgres)" && [[ -n "$pg_cid" ]] || return 1
  [[ "$(sql "${RB[@]}" "$OLD_TREE" "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname IN ('public','fvoci') AND c.relkind IN ('r','p','v','m','S')")" == 0 ]] || return 1
  project_compose "${RB[@]}" "$OLD_TREE" exec -T -e app_role=fvoci_app -e app_password="$APP_PASSWORD" postgres \
    sh -c 'exec psql -X -v ON_ERROR_STOP=1 -v app_role="$app_role" -v app_password="$app_password" -U "$POSTGRES_USER" -d "$POSTGRES_DB"' <<'SQL' || return 1
SELECT format('CREATE ROLE %I LOGIN PASSWORD %L NOSUPERUSER NOBYPASSRLS', :'app_role', :'app_password')
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = :'app_role')
\gexec
SQL
  docker cp "$BACKUP_DIR/database.dump" "${pg_cid}:/tmp/fvoci-restore.dump" || return 1
  project_compose "${RB[@]}" "$OLD_TREE" exec -T postgres sh -c \
    'pg_restore --list /tmp/fvoci-restore.dump | grep -v " SCHEMA - public " >/tmp/fvoci-restore.list' || return 1
  project_compose "${RB[@]}" "$OLD_TREE" exec -T postgres sh -c \
    'pg_restore -U "$POSTGRES_USER" -d "$POSTGRES_DB" --exit-on-error --single-transaction --no-owner --use-list=/tmp/fvoci-restore.list /tmp/fvoci-restore.dump' || return 1
  project_compose "${RB[@]}" "$OLD_TREE" exec -T postgres rm -f /tmp/fvoci-restore.dump /tmp/fvoci-restore.list || return 1
  project_compose "${RB[@]}" "$OLD_TREE" run --rm init || return 1
  # As restore.sh: the next whole second after the dump, 29 days of history.
  snapshot_at="$(date -u -d "$DUMP_AT + 1 second" +%Y-%m-%dT%H:%M:%SZ)" || return 1
  since="$(date -u -d "$snapshot_at - 29 days" +%Y-%m-%dT%H:%M:%SZ)" || return 1
  project_compose "${RB[@]}" "$OLD_TREE" run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate init \
    --recover-outbox --since "$since" --snapshot-at "$snapshot_at" \
    --apply --reason "restore into $ROLLBACK_PROJECT" --ack-external-replay || return 1
  project_compose "${RB[@]}" "$OLD_TREE" run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate init --rebuild-search
}

no_server_started() {
  [[ -z "$(service_ids "$ROLLBACK_PROJECT" server)" ]] || fail "$1: a rollback server container exists"
  ! curl -fsS -m 2 "$RB_BASE/api/v1/setup" >/dev/null 2>&1 || fail "$1: HTTP answered on $RB_BASE"
}

s3_rollback() {
  local silo_name wrong_bucket marker original fixture_size
  fixture_size="$(wc -c <"$FIXTURE_HWPX")"
  # Damage after the backup while every server is stopped, as a purge or a
  # faulty writer would: object A deleted, object B overwritten with other bytes.
  log_assert "== bucket damage after the backup: delete A, overwrite B"
  bucket_mc rm "b/${S3_BUCKET_NAME}/${KEY_A}" >/dev/null
  printf 'overwritten after the backup\n' | bucket_mc pipe "b/${S3_BUCKET_NAME}/${KEY_B}" >/dev/null
  object_versions "$KEY_A" >"$WORK/versions-a"
  object_versions "$KEY_B" >"$WORK/versions-b"
  if [[ "$(wc -l <"$WORK/versions-a")" != 2 ]] || ! grep -q ' true true$' "$WORK/versions-a"; then
    fail "A is not a delete marker over one version"
  fi
  if [[ "$(wc -l <"$WORK/versions-b")" != 2 ]] || ! grep -q " ${fixture_size} false false$" "$WORK/versions-b" \
    || grep -q " ${fixture_size} false true$" "$WORK/versions-b"; then
    fail "B latest version is not the overwrite"
  fi
  log_assert "bucket: A latest is a delete marker, B latest is ${fixture_size}-byte-mismatched overwrite, originals kept as versions: ok"

  # Point the rollback server at the same bucket: the upgrade project's silo,
  # reached through that project's network (run-owned overlay, not in the repo).
  silo_name="$(docker inspect -f '{{.Name}}' "$(running_ids "$UPGRADE_PROJECT" silo)")"
  silo_name="${silo_name#/}"
  [[ "$silo_name" =~ ^[a-z0-9][a-z0-9-]*$ ]] || fail "unexpected silo container name $silo_name"
  printf 'S3_ENDPOINT=http://%s:9000\n' "$silo_name" >>"$ROLLBACK_ENV"
  cat >"$RB_BUCKET_OVERLAY" <<EOF
services:
  server:
    networks: [default, bucket]
networks:
  bucket:
    external: true
    name: ${UPGRADE_PROJECT}_default
EOF
  log_assert "== fresh-DB restore of the pre-upgrade dump into ${ROLLBACK_PROJECT} on ${OLD_TAG}, bucket ${S3_BUCKET_NAME} via ${silo_name}"
  if ! s3_restore_db >"$WORK/restore-s3.log" 2>&1; then
    redact <"$WORK/restore-s3.log" >"$EVIDENCE_DIR/restore-s3.log"
    fail "S3 fresh-DB restore failed"
  fi
  redact <"$WORK/restore-s3.log" >"$EVIDENCE_DIR/restore-s3.log"
  [[ "$(applied_versions "${RB[@]}" "$OLD_TREE")" == "$OLD_VERSIONS" ]] || fail "restored schema is not $OLD_VERSIONS"
  no_server_started "after restore"

  # Negative controls before start: a wrong bucket aborts the check; the damaged
  # bucket is refused with exactly A missing and B size-mismatched.
  wrong_bucket="fvoci-up-absent-${RUN_ID}"
  verify_storage wrong-bucket "${RB[@]}" "$OLD_TREE" -e "S3_BUCKET=${wrong_bucket}"
  if (( VS_STATUS == 0 )) || grep -q '^{' <<<"$VS_OUT" \
    || ! grep -qF "storage probe failed: HeadBucket on \"${wrong_bucket}\"" <<<"$VS_OUT"; then
    fail "verify-storage against a wrong bucket did not fail at the bucket probe (exit ${VS_STATUS})"
  fi
  verify_storage damaged "${RB[@]}" "$OLD_TREE"
  (( VS_STATUS != 0 )) || fail "verify-storage accepted the damaged bucket"
  expect_storage_report 2 "$ATTACHMENT_ID" "$ATTACHMENT_B_ID" \
    || fail "verify-storage refused the damaged bucket with another report (exit ${VS_STATUS})"
  no_server_started "after refused verify-storage"
  log_assert "before start: wrong bucket refused at HeadBucket; damaged bucket refused (exit ${VS_STATUS}) missing=[A] sizeMismatch=[B]; no server container, no HTTP: ok"

  # Restore both objects from bucket versions (RUNNING.md item 3).
  marker="$(awk '$3 == "true" && $4 == "true" { print $1 }' "$WORK/versions-a")"
  original="$(awk -v n="$fixture_size" '$2 == n && $3 == "false" && $4 == "false" { print $1 }' "$WORK/versions-b")"
  [[ -n "$marker" && -n "$original" ]] || fail "version ids not found"
  bucket_mc rm --version-id "$marker" "b/${S3_BUCKET_NAME}/${KEY_A}" >/dev/null
  bucket_mc cp --version-id "$original" "b/${S3_BUCKET_NAME}/${KEY_B}" "b/${S3_BUCKET_NAME}/${KEY_B}" >/dev/null
  for key in "$KEY_A" "$KEY_B"; do
    object_versions "$key" | grep -q " ${fixture_size} false true$" || fail "latest version of $key is not the original size"
  done
  verify_storage restored "${RB[@]}" "$OLD_TREE"
  (( VS_STATUS == 0 )) || fail "verify-storage failed after restoring versions"
  expect_storage_report 2 "" "" || fail "unexpected verify-storage report after restoring versions"
  if ! VERIFY="$(project_compose "${RB[@]}" "$OLD_TREE" run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate server --verify-secrets 2>&1)"; then
    redact <<<"$VERIFY" >"$EVIDENCE_DIR/verify-secrets-rollback.log"
    fail "verify-secrets failed on the restored database"
  fi
  redact <<<"$VERIFY" >"$EVIDENCE_DIR/verify-secrets-rollback.log"
  no_server_started "after restored verify"
  log_assert "versions restored (A delete marker removed, B original copied back); verify-storage checked 2 ok; verify-secrets ok; still no server: ok"
  project_compose "${RB[@]}" "$OLD_TREE" up -d --wait server 2>&1 | redact >"$EVIDENCE_DIR/rollback-start.log"
  docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$(running_ids "$ROLLBACK_PROJECT" server)" \
    | grep -qx "S3_ENDPOINT=http://${silo_name}:9000" || fail "rollback server is not pointed at the original bucket"
}

# --- 6. Rollback: old checkout restore into a fresh project on the old image
RB_PORT="$(pick_port)"
RB_BASE="http://127.0.0.1:${RB_PORT}"
sed -e "s#^FVOCI_PUBLISH_PORT=.*#FVOCI_PUBLISH_PORT=${RB_PORT}#" \
  -e "s#^FVOCI_PUBLIC_ORIGIN=.*#FVOCI_PUBLIC_ORIGIN=${RB_BASE}#" "$BACKUP_ENV" >"$ROLLBACK_ENV"
chmod 600 "$ROLLBACK_ENV"
grep -qx "FVOCI_IMAGE=${OLD_TAG}" "$ROLLBACK_ENV" || fail "backup env copy does not name the old image"
RB=("$ROLLBACK_PROJECT" "$ROLLBACK_ENV")
if [[ "$STORAGE" == local ]]; then
  log_assert "== old checkout restore.sh into ${ROLLBACK_PROJECT} on ${OLD_TAG}"
  RESTORE_OUT="$(bash "$OLD_TREE/scripts/restore.sh" --project "$ROLLBACK_PROJECT" --env-file "$ROLLBACK_ENV" --input "$BACKUP_DIR" 2>&1)" \
    || { redact <<<"$RESTORE_OUT" >"$EVIDENCE_DIR/restore.log"; fail "old restore.sh failed"; }
  redact <<<"$RESTORE_OUT" >"$EVIDENCE_DIR/restore.log"
  python3 -c 'import json,sys; assert json.loads(sys.argv[1].strip().splitlines()[-1]).get("secretsVerified") is True' "$RESTORE_OUT" \
    || fail "restore did not verify secrets"
else
  s3_rollback
fi
expect_service_image "$ROLLBACK_PROJECT" server "$OLD_IMAGE_ID"
[[ "$(applied_versions "${RB[@]}" "$OLD_TREE")" == "$OLD_VERSIONS" ]] || fail "rollback schema is not $OLD_VERSIONS"
wait_http "$RB_BASE"
login "$RB_BASE"
check_seeded_data "$RB_BASE" "rollback"
[[ "$(sql "${RB[@]}" "$OLD_TREE" "$SEALED_SQL")" == 1 ]] || fail "sealed MFA secret missing after rollback"
POST_STATUS="$(curl -sS -o /dev/null -w '%{http_code}' -b "$COOKIE_JAR" \
  "$RB_BASE/api/v1/workspaces/${WORKSPACE_ID}/documents/${POST_DOC}/body")"
[[ "$POST_STATUS" == 404 ]] || fail "post-upgrade document should be absent after rollback, got HTTP $POST_STATUS"
[[ -z "$(running_ids "$UPGRADE_PROJECT" server)" ]] || fail "upgraded server running during rollback"
log_assert "rollback: old image, schema through ${OLD_VERSIONS##*,}, secrets verified, seeded data present, post-backup write absent (expected loss): ok"

log_assert "== upgrade-smoke complete ($((SECONDS - START_TS))s) old=${OLD_SHA} new=${NEW_SHA}"
