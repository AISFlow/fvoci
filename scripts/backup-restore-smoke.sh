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
MEMBER_JAR="$(mktemp "${TMPDIR:-/tmp}/fvoci-br-member-cookie.${RUN_ID}.XXXXXX")"
COLLAB_STDERR="$(mktemp "${TMPDIR:-/tmp}/fvoci-br-collab-stderr.${RUN_ID}.XXXXXX")"
DOWNLOAD_PATH="$(mktemp "${TMPDIR:-/tmp}/fvoci-br-download.${RUN_ID}.XXXXXX")"
chmod 600 "$SOURCE_ENV" "$RESTORE_ENV" "$ASSERT_LOG" "$COOKIE_JAR" "$MEMBER_JAR"
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
# Source keyring k1; the restore uses a rotated superset (k1 kept, k2 active).
ENC_K1="$(openssl rand -hex 32)"
SOURCE_ENCRYPTION_KEYS="{\"k1\":\"${ENC_K1}\"}"
RESTORE_ENCRYPTION_KEYS="{\"k1\":\"${ENC_K1}\",\"k2\":\"$(openssl rand -hex 32)\"}"
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
  if (( status != 0 )); then
    echo "== server/init logs (last 200 lines per stack)" >&2
    "${SOURCE_COMPOSE[@]}" logs --no-color --tail 200 init server >&2 || true
    "${RESTORE_COMPOSE[@]}" logs --no-color --tail 200 init server >&2 || true
  fi
  "${SOURCE_COMPOSE[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
  "${RESTORE_COMPOSE[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
  rm -rf "$BACKUP_DIR"
  rm -f "$SOURCE_ENV" "$RESTORE_ENV" "$COOKIE_JAR" "$MEMBER_JAR" "$COLLAB_STDERR" "$DOWNLOAD_PATH"
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

require_cmd docker openssl curl bun python3 sha256sum
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
  local encryption_keys="$7"
  local encryption_active="$8"
  cat >"$dest" <<EOF
FVOCI_IMAGE=${IMAGE_TAG}
POSTGRES_DB=fvoci
POSTGRES_USER=fvoci_owner
POSTGRES_PASSWORD=${owner_pw}
FVOCI_APP_ROLE=fvoci_app
FVOCI_APP_PASSWORD=${app_pw}
PASSWORD_PEPPER_KEYS=${PEPPER}
PASSWORD_PEPPER_ACTIVE_KEY_ID=install
ENCRYPTION_KEYS=${encryption_keys}
ENCRYPTION_ACTIVE_KEY_ID=${encryption_active}
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

poll_count() {
  local project="$1"
  local env_file="$2"
  local sql="$3"
  docker compose -f "$COMPOSE_FILE" --project-name "$project" --env-file "$env_file" \
    exec -T postgres psql -U fvoci_owner -d fvoci -tAc "$sql" | tr -d '[:space:]'
}

# JSON field of a reply: json_field JSON key [key|index ...].
json_field() {
  python3 -c '
import json, sys
value = json.loads(sys.argv[1])
for key in sys.argv[2:]:
    value = value[int(key)] if key.isdigit() else value[key]
print(value)
' "$@"
}

# One workspace API call with a cookie jar; prints the reply (fails on HTTP errors).
api() {
  local base="$1" jar="$2" method="$3" path="$4" body="${5:-}"
  if [[ -n "$body" ]]; then
    curl -fsS -b "$jar" -H "content-type: application/json" -H "origin: $base" \
      -X "$method" "$base/api/v1/workspaces/${WORKSPACE_ID}${path}" -d "$body"
  else
    curl -fsS -b "$jar" -H "origin: $base" -X "$method" "$base/api/v1/workspaces/${WORKSPACE_ID}${path}"
  fi
}

# What one person reads through the app role: workspaces, projects, the
# owner-only HID task status, the member task with its activity, the team
# comments with reactions, the shared wiki document's revisions, the team
# collection items, and the member's task and wiki revisions (list and
# detail: id, target, reason, creator, time, contentJson, ySnapshot). Canonical JSON (sorted keys) so equal rows compare
# equal; never prints cookies or headers.
user_oracle() {
  local base="$1" jar="$2"
  local workspaces projects hid_status task activity comments revisions query
  local task_revisions task_revision document_revision
  workspaces="$(curl -fsS -b "$jar" "$base/api/v1/me/workspaces")"
  projects="$(api "$base" "$jar" GET /projects)"
  hid_status="$(curl -sS -o /dev/null -w '%{http_code}' -b "$jar" "$base/api/v1/workspaces/${WORKSPACE_ID}/tasks/${HID_TASK_ID}")"
  task="$(api "$base" "$jar" GET "/tasks/${MEMBER_TASK_ID}")"
  activity="$(api "$base" "$jar" GET "/tasks/${MEMBER_TASK_ID}/activity")"
  comments="$(api "$base" "$jar" GET "/tasks/${OWNER_TASK_ID}/comments")"
  revisions="$(api "$base" "$jar" GET "/documents/${DOCUMENT_ID}/revisions")"
  query="$(api "$base" "$jar" POST "/collections/${TEAM_COLLECTION_ID}/query" '{"config":{"query":{"filters":{}},"groupBy":null,"dateBy":null}}')"
  task_revisions="$(api "$base" "$jar" GET "/tasks/${MEMBER_TASK_ID}/revisions")"
  task_revision="$(api "$base" "$jar" GET "/tasks/${MEMBER_TASK_ID}/revisions/${MEMBER_TASK_REVISION_ID}")"
  document_revision="$(api "$base" "$jar" GET "/documents/${DOCUMENT_ID}/revisions/${MEMBER_DOCUMENT_REVISION_ID}")"
  python3 -c '
import json, sys
workspaces, projects, hid, task, activity, comments, revisions, query = sys.argv[1:9]
task_revisions, task_revision, document_revision = sys.argv[9:12]
print(json.dumps({
    "workspaces": sorted(w["id"] for w in json.loads(workspaces)["items"]),
    "projects": sorted((p["id"], p["key"], p["name"], p["visibility"]) for p in json.loads(projects)["items"]),
    "hidTaskStatus": hid,
    "memberTask": json.loads(task),
    "activity": json.loads(activity),
    "comments": json.loads(comments),
    "revisions": json.loads(revisions),
    "teamItems": json.loads(query)["items"],
    "taskRevisions": json.loads(task_revisions),
    "memberTaskRevision": json.loads(task_revision),
    "memberDocumentRevision": json.loads(document_revision),
}, sort_keys=True))
' "$workspaces" "$projects" "$hid_status" "$task" "$activity" "$comments" "$revisions" "$query" \
    "$task_revisions" "$task_revision" "$document_revision"
}

# Protected metadata of the team rows (owner SQL): per table the row count
# and an order-independent digest of the rows. Compared, never logged.
TEAM_TABLES=(memberships project_members groups group_members document_members tasks task_assignees task_labels labels task_activity comments revisions collection_fields collection_people)
team_fingerprint() {
  local project="$1" env_file="$2" parts=() table
  for table in "${TEAM_TABLES[@]}"; do
    parts+=("SELECT '${table}:'||count(*)||':'||coalesce(md5(string_agg(md5(t::text),'' ORDER BY md5(t::text))),'') FROM fvoci.${table} t")
  done
  local sql
  sql="$(printf '%s UNION ALL ' "${parts[@]}")"
  poll_count "$project" "$env_file" "SELECT string_agg(x, ',' ORDER BY x) FROM (${sql% UNION ALL }) AS q(x)"
}

# Protected native history (owner SQL): document/task native states (with
# the snapshot cutoff / tail sequence the revision loader uses), collab
# updates and op receipts (with the writing actor) as stable projections
# (bytes as digests, no mutable timestamps), plus the immutable attachment
# metadata subset (the
# download hash is checked separately). Count + order-independent digest
# per projection; compared, never logged.
NATIVE_PROJECTIONS=(
  "document_states|SELECT workspace_id, document_id, md5(state), encoding, snapshot_cutoff_seq, tail_seq, created_at FROM fvoci.document_states"
  "document_collab_updates|SELECT workspace_id, document_id, seq, op_id, md5(payload), created_at FROM fvoci.document_collab_updates"
  "document_collab_op_receipts|SELECT workspace_id, document_id, op_id, seq, payload_len, encode(payload_sha256, 'hex'), actor_user_id, created_at FROM fvoci.document_collab_op_receipts"
  "task_states|SELECT workspace_id, task_id, md5(state), encoding, snapshot_cutoff_seq, tail_seq, created_at FROM fvoci.task_states"
  "task_collab_updates|SELECT workspace_id, task_id, seq, op_id, md5(payload), created_at FROM fvoci.task_collab_updates"
  "task_collab_op_receipts|SELECT workspace_id, task_id, op_id, seq, payload_len, encode(payload_sha256, 'hex'), actor_user_id, created_at FROM fvoci.task_collab_op_receipts"
  "attachments|SELECT id, workspace_id, document_id, name, declared_mime, size_bytes, storage_key, created_at, completed_at FROM fvoci.attachments"
)
native_fingerprint() {
  local project="$1" env_file="$2" parts=() entry
  for entry in "${NATIVE_PROJECTIONS[@]}"; do
    parts+=("SELECT '${entry%%|*}:'||count(*)||':'||coalesce(md5(string_agg(md5(x::text),'' ORDER BY md5(x::text))),'') FROM (${entry#*|}) x")
  done
  local sql
  sql="$(printf '%s UNION ALL ' "${parts[@]}")"
  poll_count "$project" "$env_file" "SELECT string_agg(y, ',' ORDER BY y) FROM (${sql% UNION ALL }) AS q(y)"
}
# Row counts only (no digests) of the native history tables, for the log.
native_counts() {
  poll_count "$1" "$2" "SELECT 'document_states='||(SELECT count(*) FROM fvoci.document_states)||';document_collab_updates='||(SELECT count(*) FROM fvoci.document_collab_updates)||';document_collab_op_receipts='||(SELECT count(*) FROM fvoci.document_collab_op_receipts)||';task_states='||(SELECT count(*) FROM fvoci.task_states)||';task_collab_updates='||(SELECT count(*) FROM fvoci.task_collab_updates)||';task_collab_op_receipts='||(SELECT count(*) FROM fvoci.task_collab_op_receipts)"
}

SOURCE_PORT="$(pick_port)"
SOURCE_BASE="http://127.0.0.1:${SOURCE_PORT}"
write_env "$SOURCE_ENV" "$OWNER_PASSWORD" "$APP_PASSWORD" "$MEILI_MASTER_KEY" "$SOURCE_BASE" "$SOURCE_PORT" \
  "$SOURCE_ENCRYPTION_KEYS" k1

python3 "$ROOT/scripts/encryption_keys.py" self-test
log_assert "encryption keyring fingerprint self-test: ok"

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

BODY_BEFORE="$(bun "$ROOT/scripts/install-smoke-collab.mjs" \
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
for bad in "--document-id ${DOCUMENT_ID} --task-id ${DOCUMENT_ID}" "" "--document-id ${DOCUMENT_ID} --fixture unknown" "--document-id ${DOCUMENT_ID} --client-id x42" "--document-id ${DOCUMENT_ID} --client-id 4294967296"; do
  # shellcheck disable=SC2086 # the bad argument set is split on purpose
  if bun "$ROOT/scripts/install-smoke-collab.mjs" --base-url "$SOURCE_BASE" --origin "$SOURCE_BASE" \
    --session "$SESSION" --workspace-id "$WORKSPACE_ID" $bad >/dev/null 2>&1; then
    echo "collab helper accepted an invalid target: ${bad}" >&2
    exit 1
  fi
done
log_assert "collab helper refuses both/neither target, an unknown fixture and invalid client ids: ok"

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
COMMENT_CREATE="$(curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $SOURCE_BASE" \
  -X POST "$SOURCE_BASE/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/comments" \
  -d '{"body":"백업 복원 댓글 🙂"}')"
COMMENT_ID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["id"])' "$COMMENT_CREATE")"
log_assert "document comment create: ok (${COMMENT_ID})"

# Team / multi-author rows through the ordinary routes: an invited second
# member, a private project with a member grant and one without, a group
# (viewer) grant on the shared wiki document, member-authored task/activity,
# a reply and a reaction, owner and member revisions (member task + shared
# wiki document; a workspace member's wiki base level is already edit, the
# effective level being max(base, group grant)) and a person value naming
# the member.
OWNER_ID="$(json_field "$LOGIN_RESPONSE" userId)"
MEMBER_EMAIL="member@backup.test"
MEMBER_PASSWORD_LOGIN="$(openssl rand -hex 12)"
INVITE="$(api "$SOURCE_BASE" "$COOKIE_JAR" POST /invitations "{\"email\":\"${MEMBER_EMAIL}\",\"role\":\"member\"}")"
INVITE_TOKEN="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["acceptUrl"].rsplit("/invite/", 1)[1])' "$INVITE")"
ACCEPTED="$(curl -fsS -c "$MEMBER_JAR" -b "$MEMBER_JAR" -H "content-type: application/json" -H "origin: $SOURCE_BASE" \
  -X POST "$SOURCE_BASE/api/v1/invitations/${INVITE_TOKEN}/accept" \
  -d "{\"email\":\"${MEMBER_EMAIL}\",\"givenName\":\"Member\",\"password\":\"${MEMBER_PASSWORD_LOGIN}\"}")"
MEMBER_ID="$(json_field "$ACCEPTED" userId)"
log_assert "second member invited and accepted: ok"
PRV_ID="$(json_field "$(api "$SOURCE_BASE" "$COOKIE_JAR" POST /projects '{"key":"PRV","name":"Private team project","visibility":"private"}')" id)"
api "$SOURCE_BASE" "$COOKIE_JAR" POST "/projects/${PRV_ID}/members" "{\"userId\":\"${MEMBER_ID}\",\"role\":\"member\"}" >/dev/null
HID_ID="$(json_field "$(api "$SOURCE_BASE" "$COOKIE_JAR" POST /projects '{"key":"HID","name":"Hidden project","visibility":"private"}')" id)"
HID_TASK_ID="$(json_field "$(api "$SOURCE_BASE" "$COOKIE_JAR" POST "/projects/${HID_ID}/tasks" '{"title":"Hidden owner task"}')" id)"
OWNER_TASK_ID="$(json_field "$(api "$SOURCE_BASE" "$COOKIE_JAR" POST "/projects/${PRV_ID}/tasks" '{"title":"Owner team task"}')" id)"
LABEL_ID="$(json_field "$(api "$SOURCE_BASE" "$COOKIE_JAR" POST "/projects/${PRV_ID}/labels" '{"name":"팀 라벨","color":"teal"}')" id)"
GROUP_ID="$(json_field "$(api "$SOURCE_BASE" "$COOKIE_JAR" POST /groups '{"name":"Backup team"}')" id)"
api "$SOURCE_BASE" "$COOKIE_JAR" POST "/groups/${GROUP_ID}/members" "{\"userId\":\"${MEMBER_ID}\"}" >/dev/null
api "$SOURCE_BASE" "$COOKIE_JAR" POST "/documents/${DOCUMENT_ID}/groups" "{\"groupId\":\"${GROUP_ID}\",\"role\":\"viewer\"}" >/dev/null
log_assert "private project grant, hidden project, group document grant: ok"
MEMBER_TASK_ID="$(json_field "$(api "$SOURCE_BASE" "$MEMBER_JAR" POST "/projects/${PRV_ID}/tasks" '{"title":"Member team task"}')" id)"
api "$SOURCE_BASE" "$MEMBER_JAR" PATCH "/tasks/${MEMBER_TASK_ID}" "{\"assigneeIds\":[\"${OWNER_ID}\"],\"labelIds\":[\"${LABEL_ID}\"]}" >/dev/null
OWNER_COMMENT_ID="$(json_field "$(api "$SOURCE_BASE" "$COOKIE_JAR" POST "/tasks/${OWNER_TASK_ID}/comments" '{"body":"오너 팀 댓글"}')" id)"
api "$SOURCE_BASE" "$MEMBER_JAR" POST "/tasks/${OWNER_TASK_ID}/comments" "{\"body\":\"멤버 답글 🙂\",\"parentId\":\"${OWNER_COMMENT_ID}\"}" >/dev/null
api "$SOURCE_BASE" "$MEMBER_JAR" POST "/comments/${OWNER_COMMENT_ID}/reactions" '{"emoji":"👍","on":true}' >/dev/null
log_assert "member task/activity, reply and reaction: ok"
api "$SOURCE_BASE" "$COOKIE_JAR" POST "/documents/${DOCUMENT_ID}/revisions" >/dev/null
TEAM_COLLECTION_ID="$(json_field "$(api "$SOURCE_BASE" "$COOKIE_JAR" GET "/projects/${PRV_ID}/collection")" id)"
TEAM_FIELD="$(api "$SOURCE_BASE" "$COOKIE_JAR" POST "/collections/${TEAM_COLLECTION_ID}/fields" '{"name":"담당 팀원","type":"user","options":[]}')"
TEAM_ITEM="$(api "$SOURCE_BASE" "$MEMBER_JAR" GET "/tasks/${MEMBER_TASK_ID}/collection-item")"
api "$SOURCE_BASE" "$MEMBER_JAR" PUT "/collections/${TEAM_COLLECTION_ID}/items/$(json_field "$TEAM_ITEM" item id)/values" \
  "{\"fieldId\":\"$(json_field "$TEAM_FIELD" id)\",\"expectedVersion\":$(json_field "$TEAM_ITEM" item version),\"expectedFieldVersion\":$(json_field "$TEAM_FIELD" version),\"value\":{\"users\":[\"${MEMBER_ID}\"]}}" >/dev/null
log_assert "document revision and member person value: ok"
# A task revision needs the task's native state: the member saves the task
# body through the ordinary collab client (persist ACK + body readback)
# first; a REST-created task has no native state yet.
MEMBER_SESSION="$(awk '$6 == "fvoci_session" { print $7; exit }' "$MEMBER_JAR")"
MEMBER_TASK_BODY="$(bun "$ROOT/scripts/install-smoke-collab.mjs" \
  --base-url "$SOURCE_BASE" \
  --origin "$SOURCE_BASE" \
  --session "$MEMBER_SESSION" \
  --workspace-id "$WORKSPACE_ID" \
  --task-id "$MEMBER_TASK_ID" \
  --client-id 43)"
if ! grep -q '"contentJson"' <<<"$MEMBER_TASK_BODY"; then
  echo "member task collab body save failed" >&2
  exit 1
fi
log_assert "member task body collab save + persist ACK + readback: ok"
MEMBER_TASK_REVISION_ID="$(json_field "$(api "$SOURCE_BASE" "$MEMBER_JAR" POST "/tasks/${MEMBER_TASK_ID}/revisions")" id)"
# An unchanged body would make the member's revision the owner's existing
# one (revision dedup keeps its creator): the member first writes a distinct
# pinned edit to the shared wiki document (persist ACK + readback); the
# restored body is then expected to equal this newer body.
# The server refuses another person's claim of a collab client id used in
# the room within its TTL. To make that deterministic, the owner first
# rejoins with client id 42 (replaying the already-integrated pinned
# fixture: the body stays the same), then the same member and document:
# with 42 the join is refused, with a distinct id it is admitted read-write
# and the edit persists.
OWNER_REJOIN="$(bun "$ROOT/scripts/install-smoke-collab.mjs" \
  --base-url "$SOURCE_BASE" \
  --origin "$SOURCE_BASE" \
  --session "$SESSION" \
  --workspace-id "$WORKSPACE_ID" \
  --document-id "$DOCUMENT_ID")"
python3 -c '
import json, sys
assert json.loads(sys.argv[1])["contentJson"] == json.loads(sys.argv[2])["contentJson"]
' "$OWNER_REJOIN" "$BODY_BEFORE"
log_assert "owner rejoins the wiki room with client id 42 (body unchanged): ok"
if bun "$ROOT/scripts/install-smoke-collab.mjs" \
  --base-url "$SOURCE_BASE" \
  --origin "$SOURCE_BASE" \
  --session "$MEMBER_SESSION" \
  --workspace-id "$WORKSPACE_ID" \
  --document-id "$DOCUMENT_ID" \
  --fixture pending_u1 >/dev/null 2>"$COLLAB_STDERR"; then
  echo "the member's claim of the owner's recent client id 42 was not refused" >&2
  exit 1
fi
if ! grep -q 'collab auth denied: not found' "$COLLAB_STDERR"; then
  echo "client id 42 join failed for another reason: $(grep -m1 -o 'Error: .*' "$COLLAB_STDERR")" >&2
  exit 1
fi
log_assert "member join with the owner's recent client id 42 refused (collab auth denied: not found): ok"
MEMBER_WIKI_BODY="$(bun "$ROOT/scripts/install-smoke-collab.mjs" \
  --base-url "$SOURCE_BASE" \
  --origin "$SOURCE_BASE" \
  --session "$MEMBER_SESSION" \
  --workspace-id "$WORKSPACE_ID" \
  --document-id "$DOCUMENT_ID" \
  --fixture pending_u1 \
  --client-id 44 2>"$COLLAB_STDERR")"
if ! grep -qx 'collab auth scope: read-write' "$COLLAB_STDERR"; then
  echo "member wiki join with client id 44 was not read-write: $(head -c 200 "$COLLAB_STDERR")" >&2
  exit 1
fi
python3 -c '
import json, sys
before, after = json.loads(sys.argv[1])["contentJson"], json.loads(sys.argv[2])["contentJson"]
assert after != before and all(node in after["content"] for node in before["content"]), (before, after)
' "$BODY_BEFORE" "$MEMBER_WIKI_BODY"
BODY_BEFORE="$MEMBER_WIKI_BODY"
log_assert "member distinct wiki edit (owner content kept) + persist ACK + readback: ok"
MEMBER_DOCUMENT_REVISION_ID="$(json_field "$(api "$SOURCE_BASE" "$MEMBER_JAR" POST "/documents/${DOCUMENT_ID}/revisions")" id)"
log_assert "member task revision and member shared wiki revision: ok"
SOURCE_OWNER_ORACLE="$(user_oracle "$SOURCE_BASE" "$COOKIE_JAR")"
SOURCE_MEMBER_ORACLE="$(user_oracle "$SOURCE_BASE" "$MEMBER_JAR")"
python3 -c '
import json, sys
owner, member, prv, hid = json.loads(sys.argv[1]), json.loads(sys.argv[2]), sys.argv[3], sys.argv[4]
assert owner["hidTaskStatus"] == "200" and member["hidTaskStatus"] == "404", (owner["hidTaskStatus"], member["hidTaskStatus"])
assert prv in [p[0] for p in member["projects"]] and hid not in [p[0] for p in member["projects"]], member["projects"]
assert hid in [p[0] for p in owner["projects"]], owner["projects"]
assert len(owner["comments"]["items"]) == 2 and len(owner["revisions"]["items"]) >= 2
for view in (owner, member):
    for key, kind, target in (("memberTaskRevision", "task", sys.argv[6]), ("memberDocumentRevision", "document", sys.argv[7])):
        rev = view[key]
        assert rev["createdBy"] == sys.argv[5] and rev["targetKind"] == kind and rev["targetId"] == target, rev
        assert isinstance(rev["contentJson"], dict) and rev["ySnapshot"], key
    assert any(r["id"] == view["memberTaskRevision"]["id"] for r in view["taskRevisions"]["items"]), view["taskRevisions"]
' "$SOURCE_OWNER_ORACLE" "$SOURCE_MEMBER_ORACLE" "$PRV_ID" "$HID_ID" "$MEMBER_ID" "$MEMBER_TASK_ID" "$DOCUMENT_ID"
log_assert "per-user source reads (member sees PRV, HID 404; owner sees both; member revisions by the member): ok"

# A secret sealed with ENCRYPTION_KEYS (MFA setup stores the TOTP secret
# sealed; it is not enabled, so password login stays single-factor).
curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $SOURCE_BASE" \
  -X POST "$SOURCE_BASE/api/v1/auth/mfa/setup" \
  -d "{\"currentPassword\":\"${OWNER_PASSWORD_LOGIN}\"}" >/dev/null
SEALED_MFA="$(poll_count "$SOURCE_PROJECT" "$SOURCE_ENV" "SELECT count(*) FROM fvoci.user_mfa WHERE totp_secret LIKE 'enc:v2:k1:%'")"
if [[ "$SEALED_MFA" != "1" ]]; then
  echo "expected one sealed MFA secret, got ${SEALED_MFA}" >&2
  exit 1
fi
log_assert "sealed MFA secret on source: ok"

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
ENC_K1="$ENC_K1" python3 - "$BACKUP_DIR/manifest.json" <<'PY'
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
keys = manifest["encryptionKeys"]
assert keys["configured"] is True, keys
assert sorted(keys["keyFingerprints"]) == ["k1"], keys
assert os.environ["ENC_K1"] not in blob, "raw ENCRYPTION_KEYS key in manifest"
PY
log_assert "backup archive private + no extra secrets, search omitted: ok ($((SECONDS - BACKUP_START))s)"
# The server stays stopped after the dump: these rows are the backed-up state.
SOURCE_TEAM_FINGERPRINT="$(team_fingerprint "$SOURCE_PROJECT" "$SOURCE_ENV")"
if [[ -z "$SOURCE_TEAM_FINGERPRINT" ]]; then
  echo "source team fingerprint is empty" >&2
  exit 1
fi
log_assert "source team metadata fingerprint taken (not printed): ok"
SOURCE_NATIVE_COUNTS="$(native_counts "$SOURCE_PROJECT" "$SOURCE_ENV")"
python3 -c '
import sys
counts = dict(item.split("=") for item in sys.argv[1].split(";"))
# Nonempty source witness: the collab-saved wiki body and member task body
# left native rows.
assert int(counts["document_states"]) + int(counts["document_collab_updates"]) >= 1, counts
assert int(counts["task_states"]) + int(counts["task_collab_updates"]) >= 1, counts
' "$SOURCE_NATIVE_COUNTS"
SOURCE_NATIVE_FINGERPRINT="$(native_fingerprint "$SOURCE_PROJECT" "$SOURCE_ENV")"
log_assert "source native history rows (${SOURCE_NATIVE_COUNTS}) fingerprint taken (not printed): ok"

log_assert "== destroy source stack and volumes"
"${SOURCE_COMPOSE[@]}" down -v --remove-orphans
if docker volume inspect "${SOURCE_PROJECT}_storage" >/dev/null 2>&1; then
  echo "source storage volume still exists after down -v" >&2
  exit 1
fi
log_assert "source stack and volumes removed: ok"

RESTORE_PORT="$(pick_port)"
RESTORE_BASE="http://127.0.0.1:${RESTORE_PORT}"
log_assert "== restore with a different pepper must be refused"
WRONG_ENV="$(mktemp "${TMPDIR:-/tmp}/fvoci-br-wrong-env.${RUN_ID}.XXXXXX")"
chmod 600 "$WRONG_ENV"
sed -E "s#^PASSWORD_PEPPER_KEYS=.*#PASSWORD_PEPPER_KEYS={\"install\":\"$(openssl rand -hex 32)\"}#" "$SOURCE_ENV" >"$WRONG_ENV"
WRONG_PROJECT="${RESTORE_PROJECT}-wrongpepper"
if bash "$ROOT/scripts/restore.sh" --project "$WRONG_PROJECT" --env-file "$WRONG_ENV" --input "$BACKUP_DIR" >/dev/null 2>&1; then
  rm -f "$WRONG_ENV"
  echo "restore with a different pepper keyring must fail" >&2
  exit 1
fi
rm -f "$WRONG_ENV"
if docker volume ls --format '{{.Name}}' | grep -q "^${WRONG_PROJECT}_"; then
  echo "refused restore must not create volumes" >&2
  exit 1
fi
log_assert "restore with a different pepper refused before touching anything: ok"

log_assert "== restore with a different key under a backed-up ENCRYPTION_KEYS id must be refused"
WRONG_ENV="$(mktemp "${TMPDIR:-/tmp}/fvoci-br-wrong-env.${RUN_ID}.XXXXXX")"
chmod 600 "$WRONG_ENV"
sed -E "s#^ENCRYPTION_KEYS=.*#ENCRYPTION_KEYS={\"k1\":\"$(openssl rand -hex 32)\"}#" "$SOURCE_ENV" >"$WRONG_ENV"
WRONG_PROJECT="${RESTORE_PROJECT}-wrongkeys"
if bash "$ROOT/scripts/restore.sh" --project "$WRONG_PROJECT" --env-file "$WRONG_ENV" --input "$BACKUP_DIR" >/dev/null 2>&1; then
  rm -f "$WRONG_ENV"
  echo "restore with a different ENCRYPTION_KEYS key must fail" >&2
  exit 1
fi
rm -f "$WRONG_ENV"
if docker volume ls --format '{{.Name}}' | grep -q "^${WRONG_PROJECT}_"; then
  echo "refused restore must not create volumes" >&2
  exit 1
fi
log_assert "restore with a mis-keyed ENCRYPTION_KEYS refused before touching anything: ok"

write_env "$RESTORE_ENV" "$RESTORE_OWNER_PASSWORD" "$RESTORE_APP_PASSWORD" \
  "$RESTORE_MEILI_MASTER_KEY" "$RESTORE_BASE" "$RESTORE_PORT" \
  "$RESTORE_ENCRYPTION_KEYS" k2

log_assert "== restore into a fresh project"
RESTORE_START=$SECONDS
RESTORE_OUT="$(bash "$ROOT/scripts/restore.sh" \
  --project "$RESTORE_PROJECT" \
  --env-file "$RESTORE_ENV" \
  --input "$BACKUP_DIR")"
printf '%s\n' "$RESTORE_OUT"
python3 -c 'import json,sys; body=json.loads(sys.argv[1].strip().splitlines()[-1]); assert body.get("secretsVerified") is True, body' "$RESTORE_OUT"
if grep -Fq "$ENC_K1" <<<"$RESTORE_OUT"; then
  echo "restore output printed an ENCRYPTION_KEYS key" >&2
  exit 1
fi
log_assert "restore compose up with a rotated superset keyring, secrets opened: ok ($((SECONDS - RESTORE_START))s) base=${RESTORE_BASE}"
if [[ "$(team_fingerprint "$RESTORE_PROJECT" "$RESTORE_ENV")" != "$SOURCE_TEAM_FINGERPRINT" ]]; then
  echo "restored team rows differ from the backed-up rows (fingerprint mismatch)" >&2
  exit 1
fi
log_assert "restored team metadata equals the backed-up rows: ok"
if [[ "$(native_fingerprint "$RESTORE_PROJECT" "$RESTORE_ENV")" != "$SOURCE_NATIVE_FINGERPRINT" ]]; then
  echo "restored native history / attachment metadata differ from the backed-up rows" >&2
  exit 1
fi
log_assert "restored native history and immutable attachment metadata equal the backed-up rows: ok"

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

COMMENTS_JSON="$(curl -fsS -b "$COOKIE_JAR" "$RESTORE_BASE/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/comments")"
python3 -c '
import json, sys
body = json.loads(sys.argv[1])
items = body.get("items", body if isinstance(body, list) else [])
assert any(c.get("id") == sys.argv[2] and c.get("body") == "백업 복원 댓글 🙂" for c in items), body
' "$COMMENTS_JSON" "$COMMENT_ID"
log_assert "restored document comment: ok"

TASK_JSON="$(curl -fsS -b "$COOKIE_JAR" "$RESTORE_BASE/api/v1/workspaces/${WORKSPACE_ID}/tasks/${TASK_ID}")"
python3 -c 'import json,sys; body=json.loads(sys.argv[1]); assert body.get("title")=="Backup restore task", body; assert body.get("id")==sys.argv[2], body' "$TASK_JSON" "$TASK_ID"
log_assert "restored task: ok"

: >"$MEMBER_JAR"
LOGIN_MEMBER="$(curl -fsS -c "$MEMBER_JAR" -b "$MEMBER_JAR" \
  -H "content-type: application/json" -H "origin: $RESTORE_BASE" \
  -X POST "$RESTORE_BASE/api/v1/auth/login" \
  -d "{\"email\":\"${MEMBER_EMAIL}\",\"password\":\"${MEMBER_PASSWORD_LOGIN}\"}")"
if [[ "$(json_field "$LOGIN_MEMBER" userId)" != "$MEMBER_ID" ]]; then
  echo "restored member login returned another user" >&2
  exit 1
fi
log_assert "second member login with original password after restore: ok"
if [[ "$(user_oracle "$RESTORE_BASE" "$COOKIE_JAR")" != "$SOURCE_OWNER_ORACLE" ]]; then
  echo "restored owner reads differ from the source" >&2
  exit 1
fi
if [[ "$(user_oracle "$RESTORE_BASE" "$MEMBER_JAR")" != "$SOURCE_MEMBER_ORACLE" ]]; then
  echo "restored member reads differ from the source" >&2
  exit 1
fi
log_assert "per-user restored reads equal the source (grants, HID 404, history, comments, revisions incl. member task/wiki revision detail, person value): ok"

# The index is derived: restore rebuilds it from PostgreSQL. Meili indexes
# asynchronously, so poll (read-only, bounded) until the task is searchable.
SEARCH_FOUND=""
for _ in $(seq 1 60); do
  SEARCH_JSON="$(curl -fsS -b "$COOKIE_JAR" "$RESTORE_BASE/api/v1/workspaces/${WORKSPACE_ID}/search?q=Backup%20restore%20task")"
  if python3 -c 'import json,sys; body=json.loads(sys.argv[1]); sys.exit(0 if any(i.get("id")==sys.argv[2] for i in body.get("items",[])) else 1)' "$SEARCH_JSON" "$TASK_ID"; then
    SEARCH_FOUND=1
    break
  fi
  sleep 1
done
if [[ -z "$SEARCH_FOUND" ]]; then
  echo "restored task is not searchable after rebuild: ${SEARCH_JSON}" >&2
  exit 1
fi
log_assert "restored search index finds the task: ok"

# Rebuilt search keeps per-person visibility: the member finds the PRV task
# and never the owner's HID task, which the owner finds.
search_has() {
  local jar="$1" query="$2" id="$3"
  python3 -c 'import json,sys; body=json.loads(sys.argv[1]); sys.exit(0 if any(i.get("id")==sys.argv[2] for i in body.get("items",[])) else 1)' \
    "$(curl -fsS -b "$jar" "$RESTORE_BASE/api/v1/workspaces/${WORKSPACE_ID}/search?q=${query}")" "$id"
}
SEARCH_FOUND=""
for _ in $(seq 1 60); do
  if search_has "$MEMBER_JAR" "Member%20team%20task" "$MEMBER_TASK_ID" && search_has "$COOKIE_JAR" "Hidden%20owner%20task" "$HID_TASK_ID"; then
    SEARCH_FOUND=1
    break
  fi
  sleep 1
done
if [[ -z "$SEARCH_FOUND" ]]; then
  echo "restored team tasks are not searchable after rebuild" >&2
  exit 1
fi
if search_has "$MEMBER_JAR" "Hidden%20owner%20task" "$HID_TASK_ID"; then
  echo "restored search shows the owner's hidden task to the member" >&2
  exit 1
fi
log_assert "restored search is per-person (member PRV yes, HID no; owner HID yes): ok"

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
