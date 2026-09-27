#!/usr/bin/env bash
# Image-to-image upgrade smoke for the infra/rust Compose install (local storage).
#
#   scripts/upgrade-smoke.sh --old <sha> --new <sha> [--main-ref origin/main]
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
# migrated database. S3 storage is not covered.
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
    *) usage ;;
  esac
done
[[ -n "$OLD_REF" && -n "$NEW_REF" ]] || usage
[[ "$BUILD_JOBS" =~ ^[1-9][0-9]*$ && "$MIN_FREE_GIB" =~ ^[0-9]+$ ]] || usage

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
  docker compose -f "$tree/infra/rust/compose.yml" --project-name "$project" --env-file "$env_file" "$@"
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
      teardown_project "$UPGRADE_PROJECT" "$UPGRADE_ENV" "$UPGRADE_TREE" || torn=1
      teardown_project "$ROLLBACK_PROJECT" "$ROLLBACK_ENV" "$OLD_TREE" || torn=1
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
  curl -fsS -b "$COOKIE_JAR" "$base/api/v1/workspaces/${WORKSPACE_ID}/attachments/${ATTACHMENT_ID}/download" -o "$DOWNLOAD_PATH"
  [[ "$(sha256sum "$DOWNLOAD_PATH" | awk '{print $1}')" == "$FIXTURE_SHA" ]] || fail "$label: attachment bytes differ"
  curl -fsS -b "$COOKIE_JAR" "$base/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/comments" \
    | python3 -c '
import json, sys
body = json.load(sys.stdin)
items = body.get("items", body if isinstance(body, list) else [])
assert any(c.get("id") == sys.argv[1] and c.get("body") == "업그레이드 댓글 🙂" for c in items), body
' "$COMMENT_ID" || fail "$label: comment missing"
  log_assert "${label}: login, document body, attachment sha256 ${FIXTURE_SHA:0:12}, comment: ok"
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
SECRETS=("$OWNER_PASSWORD" "$APP_PASSWORD" "$MEILI_MASTER_KEY" "$PEPPER_KEY" "$ENC_K1" "$OWNER_LOGIN_PASSWORD")
FIXTURE_SHA="$(sha256sum "$FIXTURE_HWPX" | awk '{print $1}')"

UP=("$UPGRADE_PROJECT" "$UPGRADE_ENV")

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
# The old checkout's own collab client, matched to the old server's API.
BODY_BEFORE="$(node "$OLD_TREE/scripts/install-smoke-collab.mjs" --base-url "$BASE" --origin "$BASE" \
  --session "$SESSION" --workspace-id "$WORKSPACE_ID" --document-id "$DOCUMENT_ID")"
grep -q '"contentJson"' <<<"$BODY_BEFORE" || fail "collab body projection failed on old image"

UPLOAD_INIT="$(curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $BASE" \
  -X POST "$BASE/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/uploads" \
  -d "{\"name\":\"sample.hwpx\",\"sizeBytes\":$(wc -c <"$FIXTURE_HWPX"),\"declaredMime\":\"application/x-hwp\"}")"
ATTACHMENT_ID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["attachmentId"])' "$UPLOAD_INIT")"
PART_URL="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["parts"][0]["url"])' "$UPLOAD_INIT")"
ETAG="$(curl -fsS -b "$COOKIE_JAR" -H "origin: $BASE" -X PUT "$BASE${PART_URL}" \
  --data-binary @"$FIXTURE_HWPX" -D - -o /dev/null | awk '/^[Ee]tag:/ { print $2; exit }' | tr -d '\r')"
curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $BASE" \
  -X POST "$BASE/api/v1/workspaces/${WORKSPACE_ID}/attachments/${ATTACHMENT_ID}/complete" \
  -d "{\"parts\":[{\"partNumber\":1,\"etag\":\"${ETAG}\"}]}" >/dev/null
EXTRACT_STATUS=pending
for _ in $(seq 1 90); do
  EXTRACT_STATUS="$(sql "${UP[@]}" "$OLD_TREE" "SELECT extract_status FROM fvoci.attachments WHERE id='${ATTACHMENT_ID}'")"
  [[ "$EXTRACT_STATUS" == pending ]] || break
  sleep 1
done
[[ "$EXTRACT_STATUS" == ok ]] || fail "old image extraction status: $EXTRACT_STATUS"
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
log_assert "== old checkout backup.sh --leave-stopped"
bash "$OLD_TREE/scripts/backup.sh" --project "$UPGRADE_PROJECT" --env-file "$UPGRADE_ENV" \
  --output "$BACKUP_DIR" --leave-stopped 2>&1 | redact >"$EVIDENCE_DIR/backup.log"
cp -p "$UPGRADE_ENV" "$BACKUP_ENV"
[[ -z "$(running_ids "$UPGRADE_PROJECT" server)" ]] || fail "server still running after backup --leave-stopped"
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
log_assert "upgraded: doctor ok, extraction kept, sealed secret opens with k1 and is invalid under another k1 (exit ${WRONG_STATUS}), new write ok: ok"

log_assert "== stop upgraded server before rollback"
UPGRADED_CID="$(running_ids "$UPGRADE_PROJECT" server)"
project_compose "${UP[@]}" "$NEW_TREE" stop -t 45 server
[[ "$(docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' "$UPGRADED_CID")" == "exited 0" ]] || fail "upgraded server did not stop cleanly"

# --- 6. Rollback: old checkout restore into a fresh project on the old image
RB_PORT="$(pick_port)"
RB_BASE="http://127.0.0.1:${RB_PORT}"
sed -e "s#^FVOCI_PUBLISH_PORT=.*#FVOCI_PUBLISH_PORT=${RB_PORT}#" \
  -e "s#^FVOCI_PUBLIC_ORIGIN=.*#FVOCI_PUBLIC_ORIGIN=${RB_BASE}#" "$BACKUP_ENV" >"$ROLLBACK_ENV"
chmod 600 "$ROLLBACK_ENV"
grep -qx "FVOCI_IMAGE=${OLD_TAG}" "$ROLLBACK_ENV" || fail "backup env copy does not name the old image"
log_assert "== old checkout restore.sh into ${ROLLBACK_PROJECT} on ${OLD_TAG}"
RESTORE_OUT="$(bash "$OLD_TREE/scripts/restore.sh" --project "$ROLLBACK_PROJECT" --env-file "$ROLLBACK_ENV" --input "$BACKUP_DIR" 2>&1)" \
  || { redact <<<"$RESTORE_OUT" >"$EVIDENCE_DIR/restore.log"; fail "old restore.sh failed"; }
redact <<<"$RESTORE_OUT" >"$EVIDENCE_DIR/restore.log"
python3 -c 'import json,sys; assert json.loads(sys.argv[1].strip().splitlines()[-1]).get("secretsVerified") is True' "$RESTORE_OUT" \
  || fail "restore did not verify secrets"
RB=("$ROLLBACK_PROJECT" "$ROLLBACK_ENV")
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
