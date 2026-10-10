#!/usr/bin/env bash
# Container install smoke for the infra/rust developer Compose stack.
#
#   FVOCI_INSTALL_IMAGE=<ref> [FVOCI_INSTALL_IMAGE_ID=<id>] scripts/install-smoke.sh
#
# The image must come from `cargo xtask install-image` (build or load) for this
# checkout; a mismatched image is refused, never rebuilt. Without
# FVOCI_INSTALL_IMAGE a local run first builds it with that command; CI refuses.
# The run owns one Compose project with a unique name; its containers, volumes
# and network are removed on success, failure and INT/TERM.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Trap glue only: phases, the first error and log groups are
# tools/install-smoke/smoke.ts; image identity, running-image and leftover
# checks are `cargo xtask install-image`.
smoke_ts() { bun "$ROOT/tools/install-smoke/smoke.ts" "$@"; }
xtask() { cargo run --quiet --locked --manifest-path "$ROOT/xtask/Cargo.toml" -- "$@"; }
json_field() { smoke_ts field "$@"; }
smoke_check() { smoke_ts check "$@"; }
SMOKE_STATE="$(mktemp "${TMPDIR:-/tmp}/install-smoke-state.XXXXXX")"
smoke_ts init "$SMOKE_STATE" install-smoke
phase() { smoke_ts phase "$SMOKE_STATE" "$1"; }
fail() { smoke_ts fail "$SMOKE_STATE" "$*"; exit 1; }
set -E
trap 'SMOKE_ERR=$?; [[ $BASHPID != "$$" ]] || smoke_ts error "$SMOKE_STATE" "$SMOKE_ERR" "$LINENO" "$BASH_COMMAND"' ERR
COMPOSE_FILE="$ROOT/infra/rust/compose.yml"
RUN_ID="$(openssl rand -hex 8)"
PROJECT="fvoci-install-smoke-${RUN_ID}"
ENV_FILE="$(mktemp "${TMPDIR:-/tmp}/fvoci-install-env.${RUN_ID}.XXXXXX")"
chmod 600 "$ENV_FILE"
COMPOSE=(docker compose -f "$COMPOSE_FILE" --project-name "$PROJECT" --env-file "$ENV_FILE")

OWNER_PASSWORD="$(openssl rand -hex 16)"
APP_PASSWORD="$(openssl rand -hex 16)"
MEILI_MASTER_KEY="$(openssl rand -hex 16)"
PEPPER="{\"install\":\"$(openssl rand -hex 32)\"}"
FIXTURE_HWPX="$ROOT/compat/fixtures/sample.hwpx"
DOCUMENT_STATE="$(mktemp "${TMPDIR:-/tmp}/fvoci-install-documents.${RUN_ID}.XXXXXX")"
ASSERT_LOG="$(mktemp "${TMPDIR:-/tmp}/fvoci-install-assert.${RUN_ID}.XXXXXX")"
COOKIE_JAR="$(mktemp "${TMPDIR:-/tmp}/fvoci-install-cookie.${RUN_ID}.XXXXXX")"
DOWNLOAD_PATH="$(mktemp "${TMPDIR:-/tmp}/fvoci-install-download.${RUN_ID}.XXXXXX")"
COLLAB_PROBE="$(mktemp "${TMPDIR:-/tmp}/fvoci-install-collab-probe.${RUN_ID}.XXXXXX")"
STACK_STARTED=0

START_TS=$SECONDS

log_assert() {
  printf '%s\n' "$1" | tee -a "$ASSERT_LOG"
}

require_cmd() {
  for cmd in "$@"; do
    command -v "$cmd" >/dev/null 2>&1 || fail "missing required command: $cmd"
  done
}

# teardown PROJECT COMPOSE...: down -v, then nothing labelled PROJECT may remain.
teardown() {
  local project="$1" out rc=0
  shift
  if ! out="$("$@" down -v --remove-orphans 2>&1)"; then
    printf 'cleanup: down failed for %s:\n%s\n' "$project" "$out" >&2
    rc=1
  fi
  xtask install-image leftovers --project "$project" || rc=1
  return "$rc"
}

cleanup() {
  local status=$?
  set +e
  trap - ERR
  smoke_ts report "$SMOKE_STATE" "$status"
  if (( status != 0 && STACK_STARTED )); then
    smoke_ts group "collect: server/init logs (last 200 lines)"
    "${COMPOSE[@]}" logs --no-color --tail 200 init server 2>&1 | smoke_ts quote >&2
    smoke_ts endgroup
  fi
  smoke_ts group "cleanup: ${PROJECT}"
  docker rm -f "fvoci-install-smoke-probe-${RUN_ID}" >/dev/null 2>&1
  if (( STACK_STARTED )); then
    teardown "$PROJECT" "${COMPOSE[@]}" || { (( status != 0 )) || status=1; }
  fi
  rm -f "$ENV_FILE" "$DOCUMENT_STATE" "$COOKIE_JAR" "$DOWNLOAD_PATH" "$COLLAB_PROBE"
  smoke_ts endgroup
  if (( status != 0 )); then
    echo "install-smoke failed after $((SECONDS - START_TS))s; assertions:" >&2
    cat "$ASSERT_LOG" >&2
  fi
  rm -f "$ASSERT_LOG"
  smoke_ts finish "$SMOKE_STATE" "$status"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

phase image
require_cmd docker openssl curl bun cargo python3 sha256sum
[[ -f "$FIXTURE_HWPX" ]] || fail "missing HWPX fixture: $FIXTURE_HWPX"

IMAGE_ENV="$(xtask install-image acquire)" || fail "install image refused or not built (see install-image above)"
IMAGE_TAG="$(sed -n 's/^FVOCI_INSTALL_IMAGE=//p' <<<"$IMAGE_ENV")"
IMAGE_ID="$(sed -n 's/^FVOCI_INSTALL_IMAGE_ID=//p' <<<"$IMAGE_ENV")"
[[ -n "$IMAGE_TAG" && -n "$IMAGE_ID" ]] || fail "install-image printed no image reference"
log_assert "image ${IMAGE_TAG} (${IMAGE_ID}) built from this checkout: ok"
HOST_PORT="$(smoke_ts port)"
BASE_URL="http://127.0.0.1:${HOST_PORT}"
ORIGIN="$BASE_URL"

cat >"$ENV_FILE" <<EOF
FVOCI_IMAGE=${IMAGE_TAG}
POSTGRES_DB=fvoci
POSTGRES_USER=fvoci_owner
POSTGRES_PASSWORD=${OWNER_PASSWORD}
FVOCI_APP_ROLE=fvoci_app
FVOCI_APP_PASSWORD=${APP_PASSWORD}
PASSWORD_PEPPER_KEYS=${PEPPER}
PASSWORD_PEPPER_ACTIVE_KEY_ID=install
FVOCI_PUBLIC_ORIGIN=${ORIGIN}
FVOCI_COOKIE_SECURE=false
FVOCI_PUBLISH_PORT=${HOST_PORT}
FVOCI_EXTRACT_POLL_SECS=2
# Collab debug logs are printed only if the smoke fails.
RUST_LOG=fvoci_server::collab=debug,collab.stage=info
MEILI_MASTER_KEY=${MEILI_MASTER_KEY}
EOF

poll_extract_field() {
  local column="$1"
  "${COMPOSE[@]}" exec -T postgres psql -U fvoci_owner -d fvoci -tAc \
    "SELECT ${column} FROM fvoci.attachments WHERE id='${ATTACHMENT_ID}'" | tr -d '[:space:]'
}

# Test the actual final image, not a host PATH with node hidden. Web assets are
# browser code. Verify required product executables, not an incidental count.
docker run --rm --name "fvoci-install-smoke-probe-${RUN_ID}" --entrypoint sh "$IMAGE_ID" -ec '
  for runtime in node nodejs bun deno qjs quickjs js d8 jsc; do
    if command -v "$runtime" >/dev/null 2>&1; then
      echo "unexpected script runtime: $runtime" >&2
      exit 1
    fi
  done
  test ! -e /opt/fvoci/node
  test ! -e /opt/fvoci/convert
  for program in fvoci-server fvoci-migrate collab-engine document-extract; do
    test -x "/opt/fvoci/bin/$program"
  done
  packages=$(dpkg-query -W -f="\${binary:Package}\n")
  if printf "%s\n" "$packages" | grep -Ei "^(nodejs|libnode|libmozjs|libjavascriptcore|quickjs)([-0-9.:]|$)"; then
    echo "unexpected script runtime package" >&2
    exit 1
  fi
'
log_assert "required product executables present; obsolete Node converter runtime absent: ok"


phase start
UP_START=$SECONDS
STACK_STARTED=1
"${COMPOSE[@]}" up -d --wait server
SERVER_CID="$("${COMPOSE[@]}" ps -q server)"
[[ -n "$SERVER_CID" ]] || fail "server container id missing"
xtask install-image running "$PROJECT" server "$IMAGE_ID" || fail "server of $PROJECT is not the one running container on $IMAGE_ID (see install-image above)"
MAPPED_PORT="$(docker port "$SERVER_CID" 8080 | head -1 | awk -F: '{print $NF}')"
if [[ "$MAPPED_PORT" != "$HOST_PORT" ]]; then
  fail "published port mismatch: expected ${HOST_PORT}, docker published ${MAPPED_PORT}"
fi
log_assert "compose up on the verified image: ok ($((SECONDS - UP_START))s) base=${BASE_URL}"

wait_http() {
  local path="$1"
  local deadline=$((SECONDS + 60))
  while (( SECONDS < deadline )); do
    if curl -fsS "$BASE_URL$path" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.5
  done
  fail "timed out waiting for $BASE_URL$path"
}

wait_http "/api/v1/setup"
log_assert "setup endpoint ready: ok"

phase test

# Run the installed diagnostic as migrate, so its converter probe must select
# the deployed server sibling and its shipped fonts rather than current_exe.
if ! DOCTOR_REPORT="$("${COMPOSE[@]}" exec -T server /opt/fvoci/bin/fvoci-migrate --doctor)"; then
  fail "installed doctor failed: $DOCTOR_REPORT"
fi
smoke_check doctor-convert "$DOCTOR_REPORT"
log_assert "installed doctor executes Rust document conversion with shipped assets: ok"

SETUP_BODY='{"email":"owner@install.test","password":"installpass1","givenName":"Owner","workspaceSlug":"install","workspaceName":"Install"}'
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" \
  -H "content-type: application/json" -H "origin: $ORIGIN" \
  -X POST "$BASE_URL/api/v1/setup" -d "$SETUP_BODY" >/dev/null
SESSION="$(awk '$6 == "fvoci_session" { print $7; exit }' "$COOKIE_JAR")"
if [[ -z "$SESSION" ]]; then
  fail "setup did not return fvoci_session cookie"
fi
log_assert "owner setup + session cookie: ok"

LOGIN_RESPONSE="$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" \
  -H "content-type: application/json" -H "origin: $ORIGIN" \
  -X POST "$BASE_URL/api/v1/auth/login" \
  -d '{"email":"owner@install.test","password":"installpass1"}')"
smoke_check user-id "$LOGIN_RESPONSE"
log_assert "password login: ok"

WORKSPACES="$(curl -fsS -b "$COOKIE_JAR" "$BASE_URL/api/v1/me/workspaces")"
WORKSPACE_ID="$(json_field "$WORKSPACES" items 0 id)"
# Choose the command/body once, preserving its identity on any replay.
DOC_COMMAND_ID="$(smoke_ts uuid)"
DOC_CREATE_BODY="{\"commandId\":\"${DOC_COMMAND_ID}\",\"parentId\":null,\"title\":\"Install doc\"}"
DOC_CREATE="$(curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $ORIGIN" \
  -X POST "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/documents" \
  -d "$DOC_CREATE_BODY")"
DOCUMENT_ID="$(json_field "$DOC_CREATE" id)"
log_assert "workspace document create: ok (${DOCUMENT_ID})"

BODY_BEFORE="$(bun "$ROOT/scripts/install-smoke-collab.mjs" \
  --base-url "$BASE_URL" \
  --origin "$ORIGIN" \
  --session "$SESSION" \
  --workspace-id "$WORKSPACE_ID" \
  --document-id "$DOCUMENT_ID")"
if ! grep -q '"contentJson"' <<<"$BODY_BEFORE"; then
  fail "collab body projection failed: $BODY_BEFORE"
fi
log_assert "collab wiki body save + projection: ok"

python3 "$ROOT/scripts/install-smoke-documents.py" "$BASE_URL" "$WORKSPACE_ID" \
  "$COOKIE_JAR" "$DOCUMENT_STATE" create
log_assert "document import/edit/exports/public PDF on the runtime image: ok"


COLLAB_STATUS="$(curl -sS -o "$COLLAB_PROBE" -w '%{http_code}' -H "origin: $ORIGIN" "$BASE_URL/collab")"
if [[ "$COLLAB_STATUS" != "426" ]]; then
  fail "expected /collab 426 when collab enabled, got $COLLAB_STATUS"
fi
log_assert "collab endpoint enabled (HTTP ${COLLAB_STATUS}, not 503): ok"

FIXTURE_SHA="$(sha256sum "$FIXTURE_HWPX" | awk '{print $1}')"
UPLOAD_INIT="$(curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $ORIGIN" \
  -X POST "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/uploads" \
  -d "{\"name\":\"sample.hwpx\",\"sizeBytes\":$(wc -c <"$FIXTURE_HWPX"),\"declaredMime\":\"application/x-hwp\"}")"
ATTACHMENT_ID="$(json_field "$UPLOAD_INIT" attachmentId)"
PART_URL="$(json_field "$UPLOAD_INIT" parts 0 url)"
PART_HEADERS="$(curl -fsS -b "$COOKIE_JAR" -H "origin: $ORIGIN" -X PUT "$BASE_URL${PART_URL}" \
  --data-binary @"$FIXTURE_HWPX" -D - -o /dev/null)"
ETAG="$(awk '/^[Ee]tag:/ { print $2; exit }' <<<"$PART_HEADERS" | tr -d '\r')"
curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $ORIGIN" \
  -X POST "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/attachments/${ATTACHMENT_ID}/complete" \
  -d "{\"parts\":[{\"partNumber\":1,\"etag\":\"${ETAG}\"}]}" >/dev/null
log_assert "attachment upload complete: ok"

EXTRACT_DEADLINE=$((SECONDS + 90))
EXTRACT_STATUS="pending"
EXTRACT_TEXT=""
while (( SECONDS < EXTRACT_DEADLINE )); do
  EXTRACT_STATUS="$(poll_extract_field extract_status)"
  if [[ "$EXTRACT_STATUS" != "pending" ]]; then
    EXTRACT_TEXT="$(poll_extract_field extract_text)"
    break
  fi
  sleep 1
done
if [[ "$EXTRACT_STATUS" == "pending" ]]; then
  fail "extraction did not finish: status=$EXTRACT_STATUS"
fi
if [[ "$EXTRACT_STATUS" != "ok" ]]; then
  fail "unexpected extract status: $EXTRACT_STATUS text=$EXTRACT_TEXT"
fi
if ! grep -q '안녕' <<<"$EXTRACT_TEXT"; then
  fail "extract text missing 안녕: $EXTRACT_TEXT"
fi
log_assert "extraction status done with expected text: ok (${EXTRACT_STATUS})"

BODY_JSON="$BODY_BEFORE"
curl -fsS -b "$COOKIE_JAR" -H "origin: $ORIGIN" \
  "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/attachments/${ATTACHMENT_ID}/download" \
  -o "$DOWNLOAD_PATH"
DOWNLOAD_SHA="$(sha256sum "$DOWNLOAD_PATH" | awk '{print $1}')"
if [[ "$DOWNLOAD_SHA" != "$FIXTURE_SHA" ]]; then
  fail "download sha256 mismatch: $DOWNLOAD_SHA != $FIXTURE_SHA"
fi
log_assert "attachment download bytes match upload sha256: ok"

RUNNING_UID="$(docker exec "$SERVER_CID" id -u)"
SERVER_PID1_UID="$(docker exec "$SERVER_CID" stat -c '%u' /proc/1)"
if [[ "$RUNNING_UID" != "1000" || "$SERVER_PID1_UID" != "1000" ]]; then
  fail "server must run as uid 1000, got exec=${RUNNING_UID} pid1=${SERVER_PID1_UID}"
fi
log_assert "server runs as non-root uid 1000: ok"
# Capture first: a failing inspect must fail the smoke, and grep -q must not
# SIGPIPE the producer under pipefail.
SERVER_ENV="$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$SERVER_CID")"
if grep -Eq '^(DATABASE_URL|FVOCI_MIGRATION_URL)=' <<<"$SERVER_ENV"; then
  fail "server container must not receive the migration owner URL"
fi
log_assert "server holds only the app database URL: ok"
if grep -Eq '^(MEILI_MASTER_KEY|FVOCI_MEILI_MASTER_KEY)=' <<<"$SERVER_ENV"; then
  fail "server container must not receive the Meilisearch master key"
fi
if ! grep -Eq '^FVOCI_MEILI_URL=' <<<"$SERVER_ENV"; then
  fail "server container missing FVOCI_MEILI_URL"
fi
if ! grep -Eq '^FVOCI_MEILI_KEY_FILE=' <<<"$SERVER_ENV"; then
  fail "server container missing FVOCI_MEILI_KEY_FILE"
fi
log_assert "server holds Meili URL and key file, not the master key: ok"
SETTINGS_JSON="$(docker exec "$SERVER_CID" sh -c 'curl -fsS -H "Authorization: Bearer $(cat /run/fvoci/meili/api_key)" http://meilisearch:7700/indexes/fvoci/settings')"
# Literal current index settings (src/search/meili.rs index_settings): seven
# searchable attributes and identifier-only displayed attributes.
smoke_check meili-settings "$SETTINGS_JSON"
log_assert "meili index settings ensured: ok"

STORAGE_SAMPLE="$(docker exec "$SERVER_CID" sh -c 'find /data/storage -type f | head -1')"
if [[ -z "$STORAGE_SAMPLE" ]]; then
  fail "storage volume has no files after upload"
fi
STORAGE_OWNER="$(docker exec "$SERVER_CID" stat -c '%u' "$STORAGE_SAMPLE")"
if [[ "$STORAGE_OWNER" != "1000" ]]; then
  fail "storage file owner expected 1000, got ${STORAGE_OWNER}"
fi
log_assert "storage files owned by service uid: ok"

phase restart
log_assert "stop server (SIGTERM), then recreate the container"
STOP_CID="$SERVER_CID"
"${COMPOSE[@]}" stop -t 45 server
# Read the exit status of the stopped container before anything restarts it;
# `docker restart` would reset State.ExitCode.
STOP_STATE="$(docker inspect -f '{{.State.Status}} {{.State.ExitCode}} {{.State.OOMKilled}}' "$STOP_CID")"
if [[ "$STOP_STATE" != "exited 0 false" ]]; then
  fail "server stop expected 'exited 0 false', got '${STOP_STATE}'"
fi
"${COMPOSE[@]}" up -d --force-recreate --wait server
SERVER_CID="$("${COMPOSE[@]}" ps -q server)"
if [[ -z "$SERVER_CID" || "$SERVER_CID" == "$STOP_CID" ]]; then
  fail "server container was not recreated (old=${STOP_CID} new=${SERVER_CID})"
fi
xtask install-image running "$PROJECT" server "$IMAGE_ID" || fail "server of $PROJECT is not the one running container on $IMAGE_ID (see install-image above)"
wait_http "/api/v1/setup"
log_assert "server SIGTERM clean exit 0 + new container on the same volumes: ok"

curl -fsS -b "$COOKIE_JAR" -H "origin: $ORIGIN" \
  "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/body" \
  | smoke_check same-body - "$BODY_JSON"

curl -fsS -b "$COOKIE_JAR" -H "origin: $ORIGIN" \
  "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/attachments/${ATTACHMENT_ID}/download" \
  -o "$DOWNLOAD_PATH"
DOWNLOAD_SHA="$(sha256sum "$DOWNLOAD_PATH" | awk '{print $1}')"
if [[ "$DOWNLOAD_SHA" != "$FIXTURE_SHA" ]]; then
  fail "post-restart download sha256 mismatch"
fi

EXTRACT_STATUS="$(poll_extract_field extract_status)"
EXTRACT_TEXT="$(poll_extract_field extract_text)"
if [[ "$EXTRACT_STATUS" != "ok" ]] || ! grep -q '안녕' <<<"$EXTRACT_TEXT"; then
  fail "post-restart extraction lost: $EXTRACT_STATUS $EXTRACT_TEXT"
fi
log_assert "post-restart doc body, attachment bytes, extraction: ok"
python3 "$ROOT/scripts/install-smoke-documents.py" "$BASE_URL" "$WORKSPACE_ID" \
  "$COOKIE_JAR" "$DOCUMENT_STATE" restart
log_assert "post-restart imported document and import job: ok"


phase collect
TOTAL=$((SECONDS - START_TS))
log_assert "install-smoke complete (${TOTAL}s)"
cat "$ASSERT_LOG"
