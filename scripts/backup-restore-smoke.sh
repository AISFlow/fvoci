#!/usr/bin/env bash
# Isolated backup/restore smoke for the infra/rust Compose install.
# Owns two Compose projects and their volumes; trap deletes only those.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="$ROOT/infra/rust/compose.yml"
IMAGE_TAG="${FVOCI_INSTALL_IMAGE:-fvoci-rust-install:local}"
RUN_ID="$(openssl rand -hex 8)"
SOURCE_PROJECT="fvoci-br-src-${RUN_ID}"
RESTORE_PROJECT="fvoci-br-dst-${RUN_ID}"
SOURCE_ENV="$(mktemp "${TMPDIR:-/tmp}/fvoci-br-src-env.${RUN_ID}.XXXXXX")"
RESTORE_ENV="$(mktemp "${TMPDIR:-/tmp}/fvoci-br-dst-env.${RUN_ID}.XXXXXX")"
BACKUP_DIR="${TMPDIR:-/tmp}/fvoci-br-backup-${RUN_ID}"
ASSERT_LOG="$(mktemp "${TMPDIR:-/tmp}/fvoci-br-assert.${RUN_ID}.XXXXXX")"
COOKIE_JAR="$(mktemp "${TMPDIR:-/tmp}/fvoci-br-cookie.${RUN_ID}.XXXXXX")"
DOWNLOAD_PATH="$(mktemp "${TMPDIR:-/tmp}/fvoci-br-download.${RUN_ID}.XXXXXX")"
chmod 600 "$SOURCE_ENV" "$RESTORE_ENV" "$ASSERT_LOG" "$COOKIE_JAR"
FIXTURE_HWPX="$ROOT/compat/fixtures/sample.hwpx"
SOURCE_COMPOSE=(docker compose -f "$COMPOSE_FILE" --project-name "$SOURCE_PROJECT" --env-file "$SOURCE_ENV")
RESTORE_COMPOSE=(docker compose -f "$COMPOSE_FILE" --project-name "$RESTORE_PROJECT" --env-file "$RESTORE_ENV")

OWNER_PASSWORD="$(openssl rand -hex 16)"
APP_PASSWORD="$(openssl rand -hex 16)"
RESTORE_OWNER_PASSWORD="$(openssl rand -hex 16)"
RESTORE_APP_PASSWORD="$(openssl rand -hex 16)"
MEILI_MASTER_KEY="$(openssl rand -hex 16)"
RESTORE_MEILI_MASTER_KEY="$(openssl rand -hex 16)"
PEPPER="{\"install\":\"$(openssl rand -hex 32)\"}"
OWNER_EMAIL="owner@backup.test"
OWNER_PASSWORD_LOGIN="installpass1"

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
  "${SOURCE_COMPOSE[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
  "${RESTORE_COMPOSE[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
  rm -rf "$BACKUP_DIR"
  rm -f "$SOURCE_ENV" "$RESTORE_ENV" "$COOKIE_JAR" "$DOWNLOAD_PATH"
  if (( status != 0 )); then
    echo "backup-restore-smoke failed after $((SECONDS - START_TS))s; assertions:" >&2
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

pick_port() {
  python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'
}

write_env() {
  local dest="$1"
  local owner_pw="$2"
  local app_pw="$3"
  local meili="$4"
  local origin="$5"
  local port="$6"
  cat >"$dest" <<EOF
FVOCI_IMAGE=${IMAGE_TAG}
POSTGRES_DB=fvoci
POSTGRES_USER=fvoci_owner
POSTGRES_PASSWORD=${owner_pw}
FVOCI_APP_ROLE=fvoci_app
FVOCI_APP_PASSWORD=${app_pw}
PASSWORD_PEPPER_KEYS=${PEPPER}
PASSWORD_PEPPER_ACTIVE_KEY_ID=install
FVOCI_PUBLIC_ORIGIN=${origin}
FVOCI_COOKIE_SECURE=false
FVOCI_PUBLISH_PORT=${port}
FVOCI_EXTRACT_POLL_SECS=2
MEILI_MASTER_KEY=${meili}
EOF
  chmod 600 "$dest"
}

wait_http() {
  local base="$1"
  local path="$2"
  local deadline=$((SECONDS + 60))
  while (( SECONDS < deadline )); do
    if curl -fsS "$base$path" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.5
  done
  echo "timed out waiting for $base$path" >&2
  return 1
}

poll_extract_field() {
  local project="$1"
  local env_file="$2"
  local column="$3"
  local attachment_id="$4"
  docker compose -f "$COMPOSE_FILE" --project-name "$project" --env-file "$env_file" \
    exec -T postgres psql -U fvoci_owner -d fvoci -tAc \
    "SELECT ${column} FROM fvoci.attachments WHERE id='${attachment_id}'" | tr -d '[:space:]'
}

SOURCE_PORT="$(pick_port)"
SOURCE_BASE="http://127.0.0.1:${SOURCE_PORT}"
write_env "$SOURCE_ENV" "$OWNER_PASSWORD" "$APP_PASSWORD" "$MEILI_MASTER_KEY" "$SOURCE_BASE" "$SOURCE_PORT"

log_assert "== build image ${IMAGE_TAG}"
BUILD_START=$SECONDS
docker build -f "$ROOT/infra/rust/Dockerfile" -t "$IMAGE_TAG" "$ROOT"
log_assert "build image: ok ($((SECONDS - BUILD_START))s)"

log_assert "== start source compose stack"
UP_START=$SECONDS
"${SOURCE_COMPOSE[@]}" up -d --wait server
log_assert "source compose up: ok ($((SECONDS - UP_START))s) base=${SOURCE_BASE}"

wait_http "$SOURCE_BASE" "/api/v1/setup"
log_assert "setup endpoint ready: ok"

SETUP_BODY="{\"email\":\"${OWNER_EMAIL}\",\"password\":\"${OWNER_PASSWORD_LOGIN}\",\"givenName\":\"Owner\",\"workspaceSlug\":\"backup\",\"workspaceName\":\"Backup\"}"
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" \
  -H "content-type: application/json" -H "origin: $SOURCE_BASE" \
  -X POST "$SOURCE_BASE/api/v1/setup" -d "$SETUP_BODY" >/dev/null
SESSION="$(awk '$6 == "fvoci_session" { print $7; exit }' "$COOKIE_JAR")"
if [[ -z "$SESSION" ]]; then
  echo "setup did not return fvoci_session cookie" >&2
  exit 1
fi
log_assert "owner setup + session cookie: ok"

LOGIN_RESPONSE="$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" \
  -H "content-type: application/json" -H "origin: $SOURCE_BASE" \
  -X POST "$SOURCE_BASE/api/v1/auth/login" \
  -d "{\"email\":\"${OWNER_EMAIL}\",\"password\":\"${OWNER_PASSWORD_LOGIN}\"}")"
python3 -c 'import json,sys; body=json.loads(sys.argv[1]); assert body.get("userId")' "$LOGIN_RESPONSE"
log_assert "password login on source: ok"

WORKSPACES="$(curl -fsS -b "$COOKIE_JAR" "$SOURCE_BASE/api/v1/me/workspaces")"
WORKSPACE_ID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["items"][0]["id"])' "$WORKSPACES")"
DOC_CREATE="$(curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $SOURCE_BASE" \
  -X POST "$SOURCE_BASE/api/v1/workspaces/${WORKSPACE_ID}/documents" \
  -d '{"parentId":null,"title":"Backup doc"}')"
DOCUMENT_ID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["id"])' "$DOC_CREATE")"
log_assert "workspace document create: ok (${DOCUMENT_ID})"

BODY_BEFORE="$(node "$ROOT/scripts/install-smoke-collab.mjs" \
  --base-url "$SOURCE_BASE" \
  --origin "$SOURCE_BASE" \
  --session "$SESSION" \
  --workspace-id "$WORKSPACE_ID" \
  --document-id "$DOCUMENT_ID")"
if ! grep -q '"contentJson"' <<<"$BODY_BEFORE"; then
  echo "collab body projection failed: $BODY_BEFORE" >&2
  exit 1
fi
log_assert "collab wiki body save + projection: ok"

FIXTURE_SHA="$(sha256sum "$FIXTURE_HWPX" | awk '{print $1}')"
UPLOAD_INIT="$(curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $SOURCE_BASE" \
  -X POST "$SOURCE_BASE/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/uploads" \
  -d "{\"name\":\"sample.hwpx\",\"sizeBytes\":$(wc -c <"$FIXTURE_HWPX"),\"declaredMime\":\"application/x-hwp\"}")"
ATTACHMENT_ID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["attachmentId"])' "$UPLOAD_INIT")"
PART_URL="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["parts"][0]["url"])' "$UPLOAD_INIT")"
ETAG="$(curl -fsS -b "$COOKIE_JAR" -H "origin: $SOURCE_BASE" -X PUT "$SOURCE_BASE${PART_URL}" \
  --data-binary @"$FIXTURE_HWPX" -D - -o /dev/null | awk '/^[Ee]tag:/ { print $2; exit }' | tr -d '\r')"
curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $SOURCE_BASE" \
  -X POST "$SOURCE_BASE/api/v1/workspaces/${WORKSPACE_ID}/attachments/${ATTACHMENT_ID}/complete" \
  -d "{\"parts\":[{\"partNumber\":1,\"etag\":\"${ETAG}\"}]}" >/dev/null
log_assert "attachment upload complete: ok"

EXTRACT_DEADLINE=$((SECONDS + 90))
EXTRACT_STATUS="pending"
EXTRACT_TEXT=""
while (( SECONDS < EXTRACT_DEADLINE )); do
  EXTRACT_STATUS="$(poll_extract_field "$SOURCE_PROJECT" "$SOURCE_ENV" extract_status "$ATTACHMENT_ID")"
  if [[ "$EXTRACT_STATUS" != "pending" ]]; then
    EXTRACT_TEXT="$(poll_extract_field "$SOURCE_PROJECT" "$SOURCE_ENV" extract_text "$ATTACHMENT_ID")"
    break
  fi
  sleep 1
done
if [[ "$EXTRACT_STATUS" != "ok" ]] || ! grep -q '안녕' <<<"$EXTRACT_TEXT"; then
  echo "extraction did not finish as expected: status=$EXTRACT_STATUS text=$EXTRACT_TEXT" >&2
  exit 1
fi
log_assert "extraction status done with expected text: ok (${EXTRACT_STATUS})"

PROJECT_CREATE="$(curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $SOURCE_BASE" \
  -X POST "$SOURCE_BASE/api/v1/workspaces/${WORKSPACE_ID}/projects" \
  -d '{"key":"BKP","name":"Backup project","visibility":"workspace"}')"
PROJECT_ID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["id"])' "$PROJECT_CREATE")"
TASK_CREATE="$(curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $SOURCE_BASE" \
  -X POST "$SOURCE_BASE/api/v1/workspaces/${WORKSPACE_ID}/projects/${PROJECT_ID}/tasks" \
  -d '{"title":"Backup restore task"}')"
TASK_ID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["id"])' "$TASK_CREATE")"
TASK_TITLE="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["title"])' "$TASK_CREATE")"
if [[ "$TASK_TITLE" != "Backup restore task" ]]; then
  echo "unexpected task title: $TASK_TITLE" >&2
  exit 1
fi
log_assert "project + task create: ok (${TASK_ID})"
log_assert "comments API is not on this main; skipped"

log_assert "== backup source stack"
BACKUP_START=$SECONDS
bash "$ROOT/scripts/backup.sh" \
  --project "$SOURCE_PROJECT" \
  --env-file "$SOURCE_ENV" \
  --output "$BACKUP_DIR" \
  --leave-stopped
MODE_DIR="$(stat -c '%a' "$BACKUP_DIR")"
MODE_DUMP="$(stat -c '%a' "$BACKUP_DIR/database.dump")"
MODE_TAR="$(stat -c '%a' "$BACKUP_DIR/storage.tar")"
MODE_MANIFEST="$(stat -c '%a' "$BACKUP_DIR/manifest.json")"
if [[ "$MODE_DIR" != "700" || "$MODE_DUMP" != "600" || "$MODE_TAR" != "600" || "$MODE_MANIFEST" != "600" ]]; then
  echo "backup permissions expected dir 700 files 600, got dir=${MODE_DIR} dump=${MODE_DUMP} tar=${MODE_TAR} manifest=${MODE_MANIFEST}" >&2
  exit 1
fi
python3 - "$BACKUP_DIR/manifest.json" <<'PY'
import json, os, sys
backup_dir = os.path.dirname(sys.argv[1])
names = sorted(os.listdir(backup_dir))
assert names == ["database.dump", "manifest.json", "storage.tar"], names
manifest = json.load(open(sys.argv[1], encoding="utf-8"))
assert manifest["search"]["included"] is False, manifest
blob = json.dumps(manifest)
assert "PASSWORD_PEPPER" not in blob
assert "MEILI_MASTER" not in blob
assert "FVOCI_APP_PASSWORD" not in blob
PY
log_assert "backup archive private + no extra secrets, search omitted: ok ($((SECONDS - BACKUP_START))s)"

log_assert "== destroy source stack and volumes"
"${SOURCE_COMPOSE[@]}" down -v --remove-orphans
if docker volume inspect "${SOURCE_PROJECT}_storage" >/dev/null 2>&1; then
  echo "source storage volume still exists after down -v" >&2
  exit 1
fi
log_assert "source stack and volumes removed: ok"

RESTORE_PORT="$(pick_port)"
RESTORE_BASE="http://127.0.0.1:${RESTORE_PORT}"
write_env "$RESTORE_ENV" "$RESTORE_OWNER_PASSWORD" "$RESTORE_APP_PASSWORD" \
  "$RESTORE_MEILI_MASTER_KEY" "$RESTORE_BASE" "$RESTORE_PORT"

log_assert "== restore into a fresh project"
RESTORE_START=$SECONDS
bash "$ROOT/scripts/restore.sh" \
  --project "$RESTORE_PROJECT" \
  --env-file "$RESTORE_ENV" \
  --input "$BACKUP_DIR"
log_assert "restore compose up: ok ($((SECONDS - RESTORE_START))s) base=${RESTORE_BASE}"

wait_http "$RESTORE_BASE" "/api/v1/setup"
: >"$COOKIE_JAR"
LOGIN_RESTORED="$(curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" \
  -H "content-type: application/json" -H "origin: $RESTORE_BASE" \
  -X POST "$RESTORE_BASE/api/v1/auth/login" \
  -d "{\"email\":\"${OWNER_EMAIL}\",\"password\":\"${OWNER_PASSWORD_LOGIN}\"}")"
python3 -c 'import json,sys; body=json.loads(sys.argv[1]); assert body.get("userId")' "$LOGIN_RESTORED"
log_assert "login with original password after restore: ok"

curl -fsS -b "$COOKIE_JAR" -H "origin: $RESTORE_BASE" \
  "$RESTORE_BASE/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/body" \
  | python3 -c 'import json,sys; body=json.load(sys.stdin); expected=json.loads(sys.argv[1]); assert body["contentJson"]==expected["contentJson"], body' "$BODY_BEFORE"
log_assert "restored wiki body matches: ok"

curl -fsS -b "$COOKIE_JAR" -H "origin: $RESTORE_BASE" \
  "$RESTORE_BASE/api/v1/workspaces/${WORKSPACE_ID}/attachments/${ATTACHMENT_ID}/download" \
  -o "$DOWNLOAD_PATH"
DOWNLOAD_SHA="$(sha256sum "$DOWNLOAD_PATH" | awk '{print $1}')"
if [[ "$DOWNLOAD_SHA" != "$FIXTURE_SHA" ]]; then
  echo "restored download sha256 mismatch: $DOWNLOAD_SHA != $FIXTURE_SHA" >&2
  exit 1
fi
log_assert "restored attachment sha256 matches: ok"

EXTRACT_STATUS="$(poll_extract_field "$RESTORE_PROJECT" "$RESTORE_ENV" extract_status "$ATTACHMENT_ID")"
EXTRACT_TEXT="$(poll_extract_field "$RESTORE_PROJECT" "$RESTORE_ENV" extract_text "$ATTACHMENT_ID")"
if [[ "$EXTRACT_STATUS" != "ok" ]] || ! grep -q '안녕' <<<"$EXTRACT_TEXT"; then
  echo "restored extraction lost: $EXTRACT_STATUS $EXTRACT_TEXT" >&2
  exit 1
fi
log_assert "restored extraction text: ok"

TASK_JSON="$(curl -fsS -b "$COOKIE_JAR" "$RESTORE_BASE/api/v1/workspaces/${WORKSPACE_ID}/tasks/${TASK_ID}")"
python3 -c 'import json,sys; body=json.loads(sys.argv[1]); assert body.get("title")=="Backup restore task", body; assert body.get("id")==sys.argv[2], body' "$TASK_JSON" "$TASK_ID"
log_assert "restored task: ok"

RESTORE_CID="$("${RESTORE_COMPOSE[@]}" ps -q server)"
RUNNING_UID="$(docker exec "$RESTORE_CID" id -u)"
SERVER_PID1_UID="$(docker exec "$RESTORE_CID" stat -c '%u' /proc/1)"
if [[ "$RUNNING_UID" != "1000" || "$SERVER_PID1_UID" != "1000" ]]; then
  echo "restored server must run as uid 1000, got exec=${RUNNING_UID} pid1=${SERVER_PID1_UID}" >&2
  exit 1
fi
log_assert "restored server runs as non-root uid 1000: ok"
SERVER_ENV="$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$RESTORE_CID")"
if grep -Eq '^(DATABASE_URL|FVOCI_MIGRATION_URL)=' <<<"$SERVER_ENV"; then
  echo "restored server must not receive the migration owner URL" >&2
  exit 1
fi
if ! grep -Eq '^DATABASE_APP_URL=' <<<"$SERVER_ENV"; then
  echo "restored server missing DATABASE_APP_URL" >&2
  exit 1
fi
log_assert "restored server holds only the app database URL: ok"
if grep -Eq '^(MEILI_MASTER_KEY|FVOCI_MEILI_MASTER_KEY)=' <<<"$SERVER_ENV"; then
  echo "restored server must not receive the Meilisearch master key" >&2
  exit 1
fi
log_assert "restored server does not hold the Meili master key: ok"

TOTAL=$((SECONDS - START_TS))
log_assert "== backup-restore-smoke complete (${TOTAL}s)"
cat "$ASSERT_LOG"
