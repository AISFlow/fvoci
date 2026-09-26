#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="$ROOT/infra/rust/compose.yml"
IMAGE_TAG="${FVOCI_INSTALL_IMAGE:-fvoci-rust-install:local}"
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
SMOKE_BIN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-install-client.${RUN_ID}.XXXXXX")"
ASSERT_LOG="$(mktemp "${TMPDIR:-/tmp}/fvoci-install-assert.${RUN_ID}.XXXXXX")"

START_TS=$SECONDS

log_assert() {
  printf '%s\n' "$1" | tee -a "$ASSERT_LOG"
}

require_cmd() {
  for cmd in "$@"; do
    command -v "$cmd" >/dev/null 2>&1 || {
      echo "missing required command: $cmd" >&2
      exit 1
    }
  done
}

cleanup() {
  local status=$?
  if (( status != 0 )); then
    echo "== server/init logs (last 200 lines)" >&2
    "${COMPOSE[@]}" logs --no-color --tail 200 init server >&2 || true
  fi
  "${COMPOSE[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
  rm -f "$ENV_FILE" "$DOCUMENT_STATE"
  rm -f "$SMOKE_BIN_DIR/install-smoke"
  rmdir "$SMOKE_BIN_DIR"
  if (( status != 0 )); then
    echo "install-smoke failed after $((SECONDS - START_TS))s; assertions:" >&2
    cat "$ASSERT_LOG" >&2 || true
  fi
  rm -f "$ASSERT_LOG"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

require_cmd docker openssl curl node python3 sha256sum

if [[ ! -f "$FIXTURE_HWPX" ]]; then
  echo "missing HWPX fixture: $FIXTURE_HWPX" >&2
  exit 1
fi

HOST_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
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

log_assert "== build image ${IMAGE_TAG}"
BUILD_START=$SECONDS
docker build -f "$ROOT/infra/rust/Dockerfile" -t "$IMAGE_TAG" "$ROOT"
log_assert "build image: ok ($((SECONDS - BUILD_START))s)"
docker build -f "$ROOT/infra/rust/Dockerfile" --target install-client \
  --output "type=local,dest=$SMOKE_BIN_DIR" "$ROOT"

# Test the actual final image, not a host PATH with node hidden. Web assets are
# browser code; only the four Rust product binaries belong in the runtime.
docker run --rm --entrypoint sh "$IMAGE_TAG" -ec '
  for runtime in node nodejs bun deno qjs quickjs js d8 jsc python python3 pypy pypy3; do
    if command -v "$runtime" >/dev/null 2>&1; then
      echo "unexpected script runtime: $runtime" >&2
      exit 1
    fi
  done
  test ! -e /opt/fvoci/node
  test ! -e /opt/fvoci/convert
  test "$(find /opt/fvoci/bin -maxdepth 1 -type f | wc -l)" -eq 4
  packages=$(dpkg-query -W -f="\${binary:Package}\n")
  if printf "%s\n" "$packages" | grep -Ei "^(nodejs|libnode|libmozjs|libjavascriptcore|quickjs|python[0-9]?|libpython[0-9]?|pypy[0-9]?)([-0-9.:]|$)"; then
    echo "unexpected script runtime package" >&2
    exit 1
  fi
'
log_assert "final product image contains no JavaScript or Python runtime: ok"


log_assert "== start compose stack"
UP_START=$SECONDS
"${COMPOSE[@]}" up -d --wait server
SERVER_CID="$("${COMPOSE[@]}" ps -q server)"
if [[ -z "$SERVER_CID" ]]; then
  echo "server container id missing" >&2
  exit 1
fi
MAPPED_PORT="$(docker port "$SERVER_CID" 8080 | head -1 | awk -F: '{print $NF}')"
if [[ "$MAPPED_PORT" != "$HOST_PORT" ]]; then
  echo "published port mismatch: expected ${HOST_PORT}, docker published ${MAPPED_PORT}" >&2
  exit 1
fi
log_assert "compose up: ok ($((SECONDS - UP_START))s) base=${BASE_URL}"

wait_http() {
  local path="$1"
  local deadline=$((SECONDS + 60))
  while (( SECONDS < deadline )); do
    if curl -fsS "$BASE_URL$path" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.5
  done
  echo "timed out waiting for $BASE_URL$path" >&2
  return 1
}

wait_http "/api/v1/setup"
log_assert "setup endpoint ready: ok"

COOKIE_JAR="$(mktemp "${TMPDIR:-/tmp}/fvoci-install-cookie.${RUN_ID}.XXXXXX")"
SETUP_BODY='{"email":"owner@install.test","password":"installpass1","givenName":"Owner","workspaceSlug":"install","workspaceName":"Install"}'
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" \
  -H "content-type: application/json" -H "origin: $ORIGIN" \
  -X POST "$BASE_URL/api/v1/setup" -d "$SETUP_BODY" >/dev/null
SESSION="$(awk '$6 == "fvoci_session" { print $7; exit }' "$COOKIE_JAR")"
if [[ -z "$SESSION" ]]; then
  echo "setup did not return fvoci_session cookie" >&2
  exit 1
fi
log_assert "owner setup + session cookie: ok"

LOGIN_RESPONSE="$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" \
  -H "content-type: application/json" -H "origin: $ORIGIN" \
  -X POST "$BASE_URL/api/v1/auth/login" \
  -d '{"email":"owner@install.test","password":"installpass1"}')"
python3 -c 'import json,sys; body=json.loads(sys.argv[1]); assert body.get("userId")' "$LOGIN_RESPONSE"
log_assert "password login: ok"

WORKSPACES="$(curl -fsS -b "$COOKIE_JAR" "$BASE_URL/api/v1/me/workspaces")"
WORKSPACE_ID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["items"][0]["id"])' "$WORKSPACES")"
DOC_CREATE="$(curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $ORIGIN" \
  -X POST "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/documents" \
  -d '{"parentId":null,"title":"Install doc"}')"
DOCUMENT_ID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["id"])' "$DOC_CREATE")"
log_assert "workspace document create: ok (${DOCUMENT_ID})"

BODY_BEFORE="$(node "$ROOT/scripts/install-smoke-collab.mjs" \
  --base-url "$BASE_URL" \
  --origin "$ORIGIN" \
  --session "$SESSION" \
  --workspace-id "$WORKSPACE_ID" \
  --document-id "$DOCUMENT_ID")"
if ! grep -q '"contentJson"' <<<"$BODY_BEFORE"; then
  echo "collab body projection failed: $BODY_BEFORE" >&2
  exit 1
fi
log_assert "collab wiki body save + projection: ok"

"$SMOKE_BIN_DIR/install-smoke" "$BASE_URL" "$WORKSPACE_ID" \
  "$COOKIE_JAR" "$DOCUMENT_STATE" create
log_assert "document import/edit/exports/public PDF on the runtime image: ok"


COLLAB_STATUS="$(curl -sS -o /tmp/fvoci-collab-probe.$$ -w '%{http_code}' -H "origin: $ORIGIN" "$BASE_URL/collab")"
rm -f "/tmp/fvoci-collab-probe.$$"
if [[ "$COLLAB_STATUS" != "426" ]]; then
  echo "expected /collab 426 when collab enabled, got $COLLAB_STATUS" >&2
  exit 1
fi
log_assert "collab endpoint enabled (HTTP ${COLLAB_STATUS}, not 503): ok"

FIXTURE_SHA="$(sha256sum "$FIXTURE_HWPX" | awk '{print $1}')"
UPLOAD_INIT="$(curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $ORIGIN" \
  -X POST "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/uploads" \
  -d "{\"name\":\"sample.hwpx\",\"sizeBytes\":$(wc -c <"$FIXTURE_HWPX"),\"declaredMime\":\"application/x-hwp\"}")"
ATTACHMENT_ID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["attachmentId"])' "$UPLOAD_INIT")"
PART_URL="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["parts"][0]["url"])' "$UPLOAD_INIT")"
ETAG="$(curl -fsS -b "$COOKIE_JAR" -H "origin: $ORIGIN" -X PUT "$BASE_URL${PART_URL}" \
  --data-binary @"$FIXTURE_HWPX" -D - -o /dev/null | awk '/^[Ee]tag:/ { print $2; exit }' | tr -d '\r')"
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
  echo "extraction did not finish: status=$EXTRACT_STATUS" >&2
  exit 1
fi
if [[ "$EXTRACT_STATUS" != "ok" ]]; then
  echo "unexpected extract status: $EXTRACT_STATUS text=$EXTRACT_TEXT" >&2
  exit 1
fi
if ! grep -q '안녕' <<<"$EXTRACT_TEXT"; then
  echo "extract text missing 안녕: $EXTRACT_TEXT" >&2
  exit 1
fi
log_assert "extraction status done with expected text: ok (${EXTRACT_STATUS})"

BODY_JSON="$BODY_BEFORE"
DOWNLOAD_PATH="$(mktemp "${TMPDIR:-/tmp}/fvoci-install-download.${RUN_ID}.XXXXXX")"
curl -fsS -b "$COOKIE_JAR" -H "origin: $ORIGIN" \
  "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/attachments/${ATTACHMENT_ID}/download" \
  -o "$DOWNLOAD_PATH"
DOWNLOAD_SHA="$(sha256sum "$DOWNLOAD_PATH" | awk '{print $1}')"
if [[ "$DOWNLOAD_SHA" != "$FIXTURE_SHA" ]]; then
  echo "download sha256 mismatch: $DOWNLOAD_SHA != $FIXTURE_SHA" >&2
  exit 1
fi
log_assert "attachment download bytes match upload sha256: ok"

RUNNING_UID="$(docker exec "$SERVER_CID" id -u)"
SERVER_PID1_UID="$(docker exec "$SERVER_CID" stat -c '%u' /proc/1)"
if [[ "$RUNNING_UID" != "1000" || "$SERVER_PID1_UID" != "1000" ]]; then
  echo "server must run as uid 1000, got exec=${RUNNING_UID} pid1=${SERVER_PID1_UID}" >&2
  exit 1
fi
log_assert "server runs as non-root uid 1000: ok"
# Capture first: a failing inspect must fail the smoke, and grep -q must not
# SIGPIPE the producer under pipefail.
SERVER_ENV="$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$SERVER_CID")"
if grep -Eq '^(DATABASE_URL|FVOCI_MIGRATION_URL)=' <<<"$SERVER_ENV"; then
  echo "server container must not receive the migration owner URL" >&2
  exit 1
fi
log_assert "server holds only the app database URL: ok"
if grep -Eq '^(MEILI_MASTER_KEY|FVOCI_MEILI_MASTER_KEY)=' <<<"$SERVER_ENV"; then
  echo "server container must not receive the Meilisearch master key" >&2
  exit 1
fi
if ! grep -Eq '^FVOCI_MEILI_URL=' <<<"$SERVER_ENV"; then
  echo "server container missing FVOCI_MEILI_URL" >&2
  exit 1
fi
if ! grep -Eq '^FVOCI_MEILI_KEY_FILE=' <<<"$SERVER_ENV"; then
  echo "server container missing FVOCI_MEILI_KEY_FILE" >&2
  exit 1
fi
log_assert "server holds Meili URL and key file, not the master key: ok"
SETTINGS_JSON="$(docker exec "$SERVER_CID" sh -c 'curl -fsS -H "Authorization: Bearer $(cat /run/fvoci/meili/api_key)" http://meilisearch:7700/indexes/fvoci/settings')"
python3 -c 'import json,sys; s=json.load(sys.stdin); assert s.get("searchableAttributes")==["title","body","chosung","stem"], s; assert "resourceKey" in s.get("filterableAttributes",[]), s' <<<"$SETTINGS_JSON"
log_assert "meili index settings ensured: ok"

STORAGE_SAMPLE="$(docker exec "$SERVER_CID" sh -c 'find /data/storage -type f | head -1')"
if [[ -z "$STORAGE_SAMPLE" ]]; then
  echo "storage volume has no files after upload" >&2
  exit 1
fi
STORAGE_OWNER="$(docker exec "$SERVER_CID" stat -c '%u' "$STORAGE_SAMPLE")"
if [[ "$STORAGE_OWNER" != "1000" ]]; then
  echo "storage file owner expected 1000, got ${STORAGE_OWNER}" >&2
  exit 1
fi
log_assert "storage files owned by service uid: ok"

log_assert "== stop server (SIGTERM), then recreate the container"
STOP_CID="$SERVER_CID"
"${COMPOSE[@]}" stop -t 45 server
# Read the exit status of the stopped container before anything restarts it;
# `docker restart` would reset State.ExitCode.
STOP_STATE="$(docker inspect -f '{{.State.Status}} {{.State.ExitCode}} {{.State.OOMKilled}}' "$STOP_CID")"
if [[ "$STOP_STATE" != "exited 0 false" ]]; then
  echo "server stop expected 'exited 0 false', got '${STOP_STATE}'" >&2
  exit 1
fi
"${COMPOSE[@]}" up -d --force-recreate --wait server
SERVER_CID="$("${COMPOSE[@]}" ps -q server)"
if [[ -z "$SERVER_CID" || "$SERVER_CID" == "$STOP_CID" ]]; then
  echo "server container was not recreated (old=${STOP_CID} new=${SERVER_CID})" >&2
  exit 1
fi
wait_http "/api/v1/setup"
log_assert "server SIGTERM clean exit 0 + new container on the same volumes: ok"

curl -fsS -b "$COOKIE_JAR" -H "origin: $ORIGIN" \
  "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/body" \
  | python3 -c 'import json,sys; body=json.load(sys.stdin); expected=json.loads(sys.argv[1]); assert body["contentJson"]==expected["contentJson"], body' "$BODY_JSON"

curl -fsS -b "$COOKIE_JAR" -H "origin: $ORIGIN" \
  "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/attachments/${ATTACHMENT_ID}/download" \
  -o "$DOWNLOAD_PATH"
DOWNLOAD_SHA="$(sha256sum "$DOWNLOAD_PATH" | awk '{print $1}')"
if [[ "$DOWNLOAD_SHA" != "$FIXTURE_SHA" ]]; then
  echo "post-restart download sha256 mismatch" >&2
  exit 1
fi

EXTRACT_STATUS="$(poll_extract_field extract_status)"
EXTRACT_TEXT="$(poll_extract_field extract_text)"
if [[ "$EXTRACT_STATUS" != "ok" ]] || ! grep -q '안녕' <<<"$EXTRACT_TEXT"; then
  echo "post-restart extraction lost: $EXTRACT_STATUS $EXTRACT_TEXT" >&2
  exit 1
fi
log_assert "post-restart doc body, attachment bytes, extraction: ok"
"$SMOKE_BIN_DIR/install-smoke" "$BASE_URL" "$WORKSPACE_ID" \
  "$COOKIE_JAR" "$DOCUMENT_STATE" restart
log_assert "post-restart imported document and import job: ok"


TOTAL=$((SECONDS - START_TS))
log_assert "== install-smoke complete (${TOTAL}s)"
cat "$ASSERT_LOG"
