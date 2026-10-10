#!/usr/bin/env bash
# Isolated backup/restore smoke for the infra/rust Compose install.
# Owns two Compose projects (plus the two refused-restore project names) and
# their volumes, all named by a per-run id; the trap deletes only those, on
# success, failure and INT/TERM, and fails a passing run if any remain.
#
#   FVOCI_INSTALL_IMAGE=<ref> [FVOCI_INSTALL_IMAGE_ID=<id>] scripts/backup-restore-smoke.sh
#
# The image must come from scripts/install-image.sh (build or load) for this
# checkout; a mismatched image is refused, never rebuilt. Without
# FVOCI_INSTALL_IMAGE a local run first builds it with that script; CI refuses.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/smoke-phases.sh
source "$ROOT/scripts/lib/smoke-phases.sh"
smoke_init backup-restore-smoke
COMPOSE_FILE="$ROOT/infra/rust/compose.yml"
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
# Optional isolated Zotero producer: a caller-supplied recipe that adds the
# db-tests Zotero fixture binary (synthetic upstream, never a product image)
# on top of the image under test. Only then do these throwaway stacks use the
# fixture's fixed synthetic keyring (read from its source); otherwise the
# install pepper and key k1 are random. Never point this at real data.
ZOTERO_FIXTURE_RECIPE="${FVOCI_BR_ZOTERO_FIXTURE_DOCKERFILE:-}"
if [[ -n "$ZOTERO_FIXTURE_RECIPE" ]]; then
  read -r PEPPER_ID PEPPER_VALUE ZOTERO_FIXTURE_KEY < <(python3 - "$ROOT" <<'PY'
import json, re, sys
root = sys.argv[1]
fixture = open(f"{root}/src/bin/e2e-fixture/zotero.rs", encoding="utf-8").read()
upstream = open(f"{root}/src/integrations/zotero.rs", encoding="utf-8").read()
(key_id, value), = json.loads(re.search(r'const PEPPER: &str =\s*r#"(.*?)"#;', fixture, re.S).group(1)).items()
api_key = re.search(r'pub const KEY: &str = "([A-Z_]+)";', upstream).group(1)
assert re.fullmatch(r"[a-z0-9]+", key_id) and re.fullmatch(r"[0-9a-f]{64}", value) and api_key.startswith("SYNTHETIC_ONLY_")
print(key_id, value, api_key)
PY
)
  if [[ -z "${ZOTERO_FIXTURE_KEY:-}" ]]; then
    fail "could not read the Zotero fixture's synthetic keyring"
  fi
  ENC_ID="$PEPPER_ID"
  ENC_K1="$PEPPER_VALUE"
else
  PEPPER_ID=install
  PEPPER_VALUE="$(openssl rand -hex 32)"
  ENC_ID=k1
  ENC_K1="$(openssl rand -hex 32)"
fi
PEPPER="{\"${PEPPER_ID}\":\"${PEPPER_VALUE}\"}"
# Source keyring ENC_ID; the restore uses a rotated superset (ENC_ID kept, k2 active).
SOURCE_ENCRYPTION_KEYS="{\"${ENC_ID}\":\"${ENC_K1}\"}"
RESTORE_ENCRYPTION_KEYS="{\"${ENC_ID}\":\"${ENC_K1}\",\"k2\":\"$(openssl rand -hex 32)\"}"
ZF_NAME="fvoci-br-zf-${RUN_ID}"
ZF_IMAGE="fvoci-br-zotero-fixture:${RUN_ID}"
ZF_ENV="$(mktemp "${TMPDIR:-/tmp}/fvoci-br-zf-env.${RUN_ID}.XXXXXX")"
ZF_STDERR="$(mktemp "${TMPDIR:-/tmp}/fvoci-br-zf-stderr.${RUN_ID}.XXXXXX")"
MOVE_FILE="$(mktemp "${TMPDIR:-/tmp}/fvoci-br-move-file.${RUN_ID}.XXXXXX")"
chmod 600 "$ZF_ENV"
OWNER_EMAIL="owner@backup.test"
OWNER_PASSWORD_LOGIN="installpass1"
# The refused restores must create nothing; their project names are torn down
# anyway so an interrupted refusal leaves nothing. `down` needs only the project
# name and an env file that sets FVOCI_IMAGE, so they use the source env file.
WRONG_PROJECTS=("${RESTORE_PROJECT}-wrongpepper" "${RESTORE_PROJECT}-wrongkeys")
SOURCE_STARTED=0
RESTORE_STARTED=0
ZF_BUILT=0

START_TS=$SECONDS

log_assert() {
  printf '%s\n' "$1" | tee -a "$ASSERT_LOG"
}

require_cmd() {
  for cmd in "$@"; do
    command -v "$cmd" >/dev/null 2>&1 || fail "missing required command: $cmd"
  done
}

cleanup() {
  local status=$? project torn=0
  set +e
  smoke_report "$status"
  if (( status != 0 )); then
    smoke_group "collect: server/init logs (last 200 lines per stack)"
    smoke_quote_begin
    (( SOURCE_STARTED )) && "${SOURCE_COMPOSE[@]}" logs --no-color --tail 200 init server >&2
    (( RESTORE_STARTED )) && "${RESTORE_COMPOSE[@]}" logs --no-color --tail 200 init server >&2
    if [[ -s "$ZF_STDERR" ]]; then
      echo "== zotero fixture stderr (last 50 lines)" >&2
      tail -n 50 "$ZF_STDERR" >&2
    fi
    smoke_quote_end
    smoke_group_end
  fi
  smoke_group "cleanup: ${SOURCE_PROJECT} ${RESTORE_PROJECT}"
  docker rm -f "$ZF_NAME" >/dev/null 2>&1
  (( ZF_BUILT )) && docker image rm "$ZF_IMAGE" >/dev/null 2>&1
  if (( SOURCE_STARTED )); then
    smoke_teardown "$SOURCE_PROJECT" "${SOURCE_COMPOSE[@]}" || torn=1
    for project in "${WRONG_PROJECTS[@]}"; do
      smoke_teardown "$project" docker compose -f "$COMPOSE_FILE" --project-name "$project" --env-file "$SOURCE_ENV" || torn=1
    done
  fi
  if (( RESTORE_STARTED )); then
    smoke_teardown "$RESTORE_PROJECT" "${RESTORE_COMPOSE[@]}" || torn=1
  fi
  (( torn == 0 || status != 0 )) || status=1
  rm -rf "$BACKUP_DIR"
  rm -f "$SOURCE_ENV" "$RESTORE_ENV" "$COOKIE_JAR" "$MEMBER_JAR" "$COLLAB_STDERR" "$DOWNLOAD_PATH" \
    "$ZF_ENV" "$ZF_STDERR" "$MOVE_FILE"
  smoke_group_end
  if (( status != 0 )); then
    echo "backup-restore-smoke failed after $((SECONDS - START_TS))s; assertions:" >&2
    cat "$ASSERT_LOG" >&2
  fi
  rm -f "$ASSERT_LOG"
  smoke_done "$status"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

phase image
require_cmd docker openssl curl bun python3 sha256sum
[[ -f "$FIXTURE_HWPX" ]] || fail "missing HWPX fixture: $FIXTURE_HWPX"

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
PASSWORD_PEPPER_ACTIVE_KEY_ID=${PEPPER_ID}
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
  fail "timed out waiting for $base$path"
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

# The same call in another workspace: api_ws BASE JAR WORKSPACE METHOD PATH [BODY].
api_ws() {
  local base="$1" jar="$2" ws="$3" method="$4" path="$5" body="${6:-}"
  if [[ -n "$body" ]]; then
    curl -fsS -b "$jar" -H "content-type: application/json" -H "origin: $base" \
      -X "$method" "$base/api/v1/workspaces/${ws}${path}" -d "$body"
  else
    curl -fsS -b "$jar" -H "origin: $base" -X "$method" "$base/api/v1/workspaces/${ws}${path}"
  fi
}

# HTTP status of one GET (no failure on 4xx).
status_of() {
  curl -sS -o /dev/null -w '%{http_code}' -b "$2" "$1$3"
}

new_uuid() {
  python3 -c 'import uuid; print(uuid.uuid4())'
}

# What one person reads through the app role: workspaces, projects, the
# owner-only HID task status, the member task with its activity, the team
# comments with reactions, the shared wiki document's revisions, the team
# collection items, and the member's task and wiki revisions (list and
# detail: id, target, reason, creator, time, contentJson, ySnapshot); the
# same-ID moved document/task with body, backlinks and the old personal
# route status; the reader's own timer history/summary of the moved and the
# member task; the wiki collection's fields, items and visible views; the
# team wiki backlinks; the owner's Zotero mirror (other readers: status).
# Canonical JSON (sorted keys, serverNow dropped) so equal rows compare
# equal; never prints cookies or headers.
user_oracle() {
  local base="$1" jar="$2"
  local workspaces projects hid_status task activity comments revisions query
  local task_revisions task_revision document_revision
  local moved_document moved_body moved_revisions moved_task moved_backlinks personal_status
  local moved_history member_history member_summary wiki_fields wiki_items wiki_views
  local document_backlinks member_backlinks zotero range
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
  range="from=${TIMER_FROM}&to=${TIMER_TO}"
  # The moved document is now a PRV project document.
  moved_document="$(api "$base" "$jar" GET "/projects/${PRV_ID}/documents/${MOVED_DOC_ID}")"
  moved_body="$(api "$base" "$jar" GET "/projects/${PRV_ID}/documents/${MOVED_DOC_ID}/body")"
  moved_revisions="[$(api "$base" "$jar" GET "/projects/${PRV_ID}/documents/${MOVED_DOC_ID}/revisions"),$(api "$base" "$jar" GET "/tasks/${MOVED_TASK_ID}/revisions")]"
  moved_task="$(api "$base" "$jar" GET "/tasks/${MOVED_TASK_ID}")"
  moved_backlinks="$(api "$base" "$jar" GET "/tasks/${MOVED_TASK_ID}/backlinks")"
  personal_status="$(status_of "$base" "$jar" "/api/v1/workspaces/${PERSONAL_ID}/tasks/${MOVED_TASK_ID}")"
  moved_history="$(api "$base" "$jar" GET "/tasks/${MOVED_TASK_ID}/timer/history?${range}")"
  member_history="$(api "$base" "$jar" GET "/tasks/${MEMBER_TASK_ID}/timer/history?${range}")"
  member_summary="$(api "$base" "$jar" GET "/tasks/${MEMBER_TASK_ID}/timer/summary?${range}")"
  wiki_fields="$(api "$base" "$jar" GET "/collections/${WIKI_COLLECTION_ID}/fields")"
  wiki_items="$(api "$base" "$jar" POST "/collections/${WIKI_COLLECTION_ID}/query" '{"config":{"query":{"filters":{}},"groupBy":null,"dateBy":null}}')"
  wiki_views="$(api "$base" "$jar" GET "/collections/${WIKI_COLLECTION_ID}/views")"
  document_backlinks="$(api "$base" "$jar" GET "/documents/${DOCUMENT_ID}/backlinks")"
  member_backlinks="$(api "$base" "$jar" GET "/tasks/${MEMBER_TASK_ID}/backlinks")"
  if [[ "$jar" == "$COOKIE_JAR" ]]; then
    zotero="$(api_ws "$base" "$jar" "$PERSONAL_ID" GET /zotero)"
    if [[ -n "$ZOTERO_CONNECTOR_ID" ]]; then
      zotero="[${zotero},$(api_ws "$base" "$jar" "$PERSONAL_ID" GET "/zotero/libraries/${ZOTERO_CONNECTOR_ID}")]"
    fi
  else
    zotero="\"$(status_of "$base" "$jar" "/api/v1/workspaces/${PERSONAL_ID}/zotero")\""
  fi
  python3 -c '
import json, sys
workspaces, projects, hid, task, activity, comments, revisions, query = sys.argv[1:9]
task_revisions, task_revision, document_revision = sys.argv[9:12]
(moved_document, moved_body, moved_revisions, moved_task, moved_backlinks, personal_status, moved_history,
 member_history, member_summary, wiki_fields, wiki_items, wiki_views, document_backlinks,
 member_backlinks, zotero) = sys.argv[12:27]
def stable(value):
    if isinstance(value, dict):
        return {k: stable(v) for k, v in value.items() if k != "serverNow"}
    if isinstance(value, list):
        return [stable(v) for v in value]
    return value
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
    "movedDocument": json.loads(moved_document),
    "movedBody": json.loads(moved_body),
    "movedRevisions": json.loads(moved_revisions),
    "movedTask": json.loads(moved_task),
    "movedBacklinks": json.loads(moved_backlinks),
    "personalMovedTaskStatus": personal_status,
    "movedTimerHistory": stable(json.loads(moved_history)),
    "memberTaskTimerHistory": stable(json.loads(member_history)),
    "memberTaskTimerSummary": stable(json.loads(member_summary)),
    "wikiFields": json.loads(wiki_fields),
    "wikiItems": json.loads(wiki_items)["items"],
    "wikiViews": json.loads(wiki_views),
    "documentBacklinks": json.loads(document_backlinks),
    "memberTaskBacklinks": json.loads(member_backlinks),
    "zotero": json.loads(zotero),
}, sort_keys=True))
' "$workspaces" "$projects" "$hid_status" "$task" "$activity" "$comments" "$revisions" "$query" \
    "$task_revisions" "$task_revision" "$document_revision" \
    "$moved_document" "$moved_body" "$moved_revisions" "$moved_task" "$moved_backlinks" "$personal_status" "$moved_history" \
    "$member_history" "$member_summary" "$wiki_fields" "$wiki_items" "$wiki_views" "$document_backlinks" \
    "$member_backlinks" "$zotero"
}

# Protected metadata of the team rows (owner SQL): per table the row count
# and an order-independent digest of the rows. Compared, never logged.
TEAM_TABLES=(memberships project_members groups group_members document_members tasks task_assignees task_labels labels task_activity comments revisions collection_fields collection_people)
# The current models (same as the team tables, whole rows): documents (moved
# and referencing bodies), the same-ID transfer receipt and task origin, the
# timer runs/segments/commands/audit and time entries, the wiki collection
# with options, choices, values and views, and the Zotero mirror including
# the sealed credential row (digest only; operator backups are whole-install).
MODEL_TABLES=(documents personal_transfer_commands task_origins time_entries task_timer_runs task_timer_segments task_timer_commands task_timer_audit task_timer_legacy_open collections collection_items collection_options collection_choices collection_values collection_views zotero_connectors zotero_credentials zotero_references zotero_collections zotero_memberships zotero_links)
team_fingerprint() {
  local project="$1" env_file="$2" parts=() table
  shift 2
  local tables=("$@")
  (( ${#tables[@]} > 0 )) || tables=("${TEAM_TABLES[@]}")
  for table in "${tables[@]}"; do
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

if [[ -n "${FVOCI_INSTALL_IMAGE:-}" ]]; then
  IMAGE_ENV="$(bash "$ROOT/scripts/install-image.sh" verify "$FVOCI_INSTALL_IMAGE" ${FVOCI_INSTALL_IMAGE_ID:+"$FVOCI_INSTALL_IMAGE_ID"})"
elif smoke_actions; then
  fail "FVOCI_INSTALL_IMAGE is required in CI: the image is built once per arch by scripts/install-image.sh; this smoke does not build it"
else
  log_assert "FVOCI_INSTALL_IMAGE unset: building the image for this checkout (scripts/install-image.sh build)"
  IMAGE_ENV="$(bash "$ROOT/scripts/install-image.sh" build)"
fi
IMAGE_TAG="$(sed -n 's/^FVOCI_INSTALL_IMAGE=//p' <<<"$IMAGE_ENV")"
IMAGE_ID="$(sed -n 's/^FVOCI_INSTALL_IMAGE_ID=//p' <<<"$IMAGE_ENV")"
[[ -n "$IMAGE_TAG" && -n "$IMAGE_ID" ]] || fail "install-image.sh printed no image reference"
log_assert "image ${IMAGE_TAG} (${IMAGE_ID}) built from this checkout: ok"
if [[ -n "$ZOTERO_FIXTURE_RECIPE" ]]; then
  log_assert "build isolated Zotero fixture image on ${IMAGE_TAG}"
  BUILD_START=$SECONDS
  ZF_BUILT=1
  docker build -f "$ZOTERO_FIXTURE_RECIPE" --build-arg "FVOCI_IMAGE=${IMAGE_TAG}" -t "$ZF_IMAGE" "$ROOT"
  # Exactly the image under test plus one layer (the fixture binary).
  python3 - "$(docker image inspect -f '{{json .RootFS.Layers}}' "$IMAGE_ID")" \
    "$(docker image inspect -f '{{json .RootFS.Layers}}' "$ZF_IMAGE")" <<'PY'
import json, sys
base, fixture = json.loads(sys.argv[1]), json.loads(sys.argv[2])
assert fixture[:len(base)] == base and len(fixture) == len(base) + 1, (len(base), len(fixture))
PY
  log_assert "fixture image is the image under test plus one fixture layer: ok ($((SECONDS - BUILD_START))s)"
fi

SOURCE_PORT="$(pick_port)"
SOURCE_BASE="http://127.0.0.1:${SOURCE_PORT}"
write_env "$SOURCE_ENV" "$OWNER_PASSWORD" "$APP_PASSWORD" "$MEILI_MASTER_KEY" "$SOURCE_BASE" "$SOURCE_PORT" \
  "$SOURCE_ENCRYPTION_KEYS" "$ENC_ID"

python3 "$ROOT/scripts/encryption_keys.py" self-test
log_assert "encryption keyring fingerprint self-test: ok"

phase start-source
UP_START=$SECONDS
SOURCE_STARTED=1
"${SOURCE_COMPOSE[@]}" up -d --wait server
expect_service_image "$SOURCE_PROJECT" server "$IMAGE_ID"
log_assert "source compose up on the verified image: ok ($((SECONDS - UP_START))s) base=${SOURCE_BASE}"

wait_http "$SOURCE_BASE" "/api/v1/setup"
log_assert "setup endpoint ready: ok"

phase seed
SETUP_BODY="{\"email\":\"${OWNER_EMAIL}\",\"password\":\"${OWNER_PASSWORD_LOGIN}\",\"givenName\":\"Owner\",\"workspaceSlug\":\"backup\",\"workspaceName\":\"Backup\"}"
curl -fsS -c "$COOKIE_JAR" -b "$COOKIE_JAR" \
  -H "content-type: application/json" -H "origin: $SOURCE_BASE" \
  -X POST "$SOURCE_BASE/api/v1/setup" -d "$SETUP_BODY" >/dev/null
SESSION="$(awk '$6 == "fvoci_session" { print $7; exit }' "$COOKIE_JAR")"
if [[ -z "$SESSION" ]]; then
  fail "setup did not return fvoci_session cookie"
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
# Choose the command/body once, preserving its identity on any replay.
DOC_COMMAND_ID="$(new_uuid)"
DOC_CREATE_BODY="{\"commandId\":\"${DOC_COMMAND_ID}\",\"parentId\":null,\"title\":\"Backup doc\"}"
DOC_CREATE="$(curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $SOURCE_BASE" \
  -X POST "$SOURCE_BASE/api/v1/workspaces/${WORKSPACE_ID}/documents" \
  -d "$DOC_CREATE_BODY")"
DOCUMENT_ID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["id"])' "$DOC_CREATE")"
log_assert "workspace document create: ok (${DOCUMENT_ID})"

BODY_BEFORE="$(bun "$ROOT/scripts/install-smoke-collab.mjs" \
  --base-url "$SOURCE_BASE" \
  --origin "$SOURCE_BASE" \
  --session "$SESSION" \
  --workspace-id "$WORKSPACE_ID" \
  --document-id "$DOCUMENT_ID")"
if ! grep -q '"contentJson"' <<<"$BODY_BEFORE"; then
  fail "collab body projection failed: $BODY_BEFORE"
fi
log_assert "collab wiki body save + projection: ok"
for bad in "--document-id ${DOCUMENT_ID} --task-id ${DOCUMENT_ID}" "" "--document-id ${DOCUMENT_ID} --fixture unknown" "--document-id ${DOCUMENT_ID} --client-id x42" "--document-id ${DOCUMENT_ID} --client-id 4294967296"; do
  # shellcheck disable=SC2086 # the bad argument set is split on purpose
  if bun "$ROOT/scripts/install-smoke-collab.mjs" --base-url "$SOURCE_BASE" --origin "$SOURCE_BASE" \
    --session "$SESSION" --workspace-id "$WORKSPACE_ID" $bad >/dev/null 2>&1; then
    fail "collab helper accepted an invalid target: ${bad}"
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
  --data-binary @"$FIXTURE_HWPX" -D - -o /dev/null | awk '/^[Ee]tag:/ && !etag { etag = $2 } END { print etag }' | tr -d '\r')"
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
  fail "extraction did not finish as expected: status=$EXTRACT_STATUS text=$EXTRACT_TEXT"
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
  fail "unexpected task title: $TASK_TITLE"
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
  fail "member task collab body save failed"
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
  fail "the member's claim of the owner's recent client id 42 was not refused"
fi
if ! grep -q 'collab auth denied: not found' "$COLLAB_STDERR"; then
  fail "client id 42 join failed for another reason: $(grep -m1 -o 'Error: .*' "$COLLAB_STDERR")"
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
  fail "member wiki join with client id 44 was not read-write: $(head -c 200 "$COLLAB_STDERR")"
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

# Current models through the ordinary routes (W2/W5/wiki/refs/files):
# the owner's personal capture gets a file on the task, a native task body,
# a native document body naming the task and the file, a revision of each
# and a stopped timer run; an explicit same-ID MOVE then puts the whole
# graph into PRV. The member writes a wiki document whose native body names
# the shared wiki document, the member task and the moved task, and runs
# their own timer; a wiki collection gets choice/person values and a shared
# (owner) and a private (member) view.
TIMER_FROM="$(date -u -d '-2 day' +%F)"
TIMER_TO="$(date -u -d '+2 day' +%F)"
ZOTERO_CONNECTOR_ID=""
PERSONAL_ID="$(json_field "$(curl -fsS -b "$COOKIE_JAR" -H "origin: $SOURCE_BASE" -X POST "$SOURCE_BASE/api/v1/me/personal-workspace")" id)"
PAIR="$(api_ws "$SOURCE_BASE" "$COOKIE_JAR" "$PERSONAL_ID" POST /personal-input "{\"requestId\":\"$(new_uuid)\",\"intent\":\"task\",\"title\":\"개인 캡처 이동 작업\"}")"
MOVED_DOC_ID="$(json_field "$PAIR" documentId)"
MOVED_TASK_ID="$(json_field "$PAIR" taskId)"
printf 'W7 operator moved file 한글🙂\n' >"$MOVE_FILE"
MOVE_UPLOAD="$(api_ws "$SOURCE_BASE" "$COOKIE_JAR" "$PERSONAL_ID" POST "/tasks/${MOVED_TASK_ID}/uploads" \
  "{\"name\":\"이동 증빙.txt\",\"sizeBytes\":$(wc -c <"$MOVE_FILE")}")"
MOVED_ATTACHMENT_ID="$(json_field "$MOVE_UPLOAD" attachmentId)"
MOVE_ETAG="$(curl -fsS -b "$COOKIE_JAR" -H "origin: $SOURCE_BASE" -H "content-type: application/octet-stream" \
  -X PUT "$SOURCE_BASE$(json_field "$MOVE_UPLOAD" parts 0 url)" --data-binary @"$MOVE_FILE" -D - -o /dev/null \
  | awk '/^[Ee]tag:/ && !etag { etag = $2 } END { print etag }' | tr -d '\r')"
api_ws "$SOURCE_BASE" "$COOKIE_JAR" "$PERSONAL_ID" POST "/attachments/${MOVED_ATTACHMENT_ID}/complete" \
  "{\"parts\":[{\"partNumber\":1,\"etag\":\"${MOVE_ETAG}\"}]}" >/dev/null
MOVED_DOC_BODY="$(python3 -c '
import json, sys
task, attachment = sys.argv[1:3]
print(json.dumps({"contentJson": {"type": "doc", "content": [
    {"type": "paragraph", "content": [
        {"type": "text", "text": "개인 문서 본문 "},
        {"type": "mention", "attrs": {"entity": "task", "id": task, "label": "이동 작업"}}]},
    {"type": "attachment", "attrs": {"id": attachment, "name": "이동 증빙.txt"}}]}}))
' "$MOVED_TASK_ID" "$MOVED_ATTACHMENT_ID")"
api_ws "$SOURCE_BASE" "$COOKIE_JAR" "$PERSONAL_ID" PUT "/documents/${MOVED_DOC_ID}/body" "$MOVED_DOC_BODY" >/dev/null
MOVED_TASK_BODY="$(bun "$ROOT/scripts/install-smoke-collab.mjs" \
  --base-url "$SOURCE_BASE" \
  --origin "$SOURCE_BASE" \
  --session "$SESSION" \
  --workspace-id "$PERSONAL_ID" \
  --task-id "$MOVED_TASK_ID")"
if ! grep -q '"contentJson"' <<<"$MOVED_TASK_BODY"; then
  fail "personal task collab body save failed"
fi
NATIVE_DEADLINE=$((SECONDS + 30))
until [[ "$(poll_count "$SOURCE_PROJECT" "$SOURCE_ENV" "SELECT (SELECT count(*) FROM fvoci.document_states WHERE document_id='${MOVED_DOC_ID}')+(SELECT count(*) FROM fvoci.document_collab_updates WHERE document_id='${MOVED_DOC_ID}')")" != "0" ]]; do
  if (( SECONDS >= NATIVE_DEADLINE )); then
    fail "personal document body left no native rows"
  fi
  sleep 0.5
done
api_ws "$SOURCE_BASE" "$COOKIE_JAR" "$PERSONAL_ID" POST "/documents/${MOVED_DOC_ID}/revisions" >/dev/null
api_ws "$SOURCE_BASE" "$COOKIE_JAR" "$PERSONAL_ID" POST "/tasks/${MOVED_TASK_ID}/revisions" >/dev/null
log_assert "personal capture with task file, native task/document bodies (task mention + file) and revisions: ok"
# timer_run BASE JAR WORKSPACE TASK: start, at least 1.1 s, stop; prints the run id.
timer_run() {
  local base="$1" jar="$2" ws="$3" task="$4" me started command
  # Called in a command substitution (no errexit there): every step returns.
  me="$(curl -fsS -b "$jar" "$base/api/v1/auth/me")" || return 1
  command="{\"expectedActorId\":\"$(json_field "$me" userId)\",\"expectedSessionId\":\"$(json_field "$me" sessionId)\"" || return 1
  started="$(api_ws "$base" "$jar" "$ws" POST "/tasks/${task}/timer" \
    "${command},\"requestId\":\"$(new_uuid)\",\"runId\":null,\"operation\":\"start\",\"expectedVersion\":0}")" || return 1
  sleep 1.2
  api_ws "$base" "$jar" "$ws" POST "/tasks/${task}/timer" \
    "${command},\"requestId\":\"$(new_uuid)\",\"runId\":\"$(json_field "$started" runId)\",\"operation\":\"stop\",\"expectedVersion\":$(json_field "$started" version)}" >/dev/null || return 1
  json_field "$started" runId
}
OWNER_RUN_ID="$(timer_run "$SOURCE_BASE" "$COOKIE_JAR" "$PERSONAL_ID" "$MOVED_TASK_ID")"
log_assert "owner timer run on the personal task (start/stop): ok"
PRV_STATUS_ID="$(json_field "$(api "$SOURCE_BASE" "$COOKIE_JAR" GET "/projects/${PRV_ID}/workflow")" statuses 0 id)"
MOVE_SELECTION="$(python3 -c '
import json, sys
doc, task, doc_version, task_version, ws, project, status = sys.argv[1:8]
print(json.dumps({"action": "move", "documentId": doc, "taskId": task,
    "expectedDocumentVersion": int(doc_version), "expectedTaskVersion": int(task_version),
    "destinationWorkspaceId": ws, "destinationProjectId": project, "destinationStatusId": status}))
' "$MOVED_DOC_ID" "$MOVED_TASK_ID" \
  "$(json_field "$(api_ws "$SOURCE_BASE" "$COOKIE_JAR" "$PERSONAL_ID" GET "/documents/${MOVED_DOC_ID}")" version)" \
  "$(json_field "$(api_ws "$SOURCE_BASE" "$COOKIE_JAR" "$PERSONAL_ID" GET "/tasks/${MOVED_TASK_ID}")" version)" \
  "$WORKSPACE_ID" "$PRV_ID" "$PRV_STATUS_ID")"
MOVE_PREVIEW="$(api_ws "$SOURCE_BASE" "$COOKIE_JAR" "$PERSONAL_ID" POST /personal-transfers/preview "$MOVE_SELECTION")"
MOVED="$(api_ws "$SOURCE_BASE" "$COOKIE_JAR" "$PERSONAL_ID" POST /personal-transfers \
  "{\"requestId\":\"$(new_uuid)\",\"selection\":${MOVE_SELECTION},\"previewDigest\":\"$(json_field "$MOVE_PREVIEW" digest)\",\"confirmed\":true}")"
python3 -c '
import json, sys
moved = json.loads(sys.argv[1])
want = dict(zip(["workspaceId", "projectId", "documentId", "taskId"], sys.argv[2:6]))
assert {k: moved[k] for k in want} == want and moved["replayed"] is False, moved
' "$MOVED" "$WORKSPACE_ID" "$PRV_ID" "$MOVED_DOC_ID" "$MOVED_TASK_ID"
log_assert "explicit same-ID MOVE of the personal graph into PRV (same document/task ids): ok"
REF_DOC_COMMAND_ID="$(new_uuid)"
REF_DOC_CREATE_BODY="{\"commandId\":\"${REF_DOC_COMMAND_ID}\",\"parentId\":null,\"title\":\"멤버 참조 문서\"}"
REF_DOC_ID="$(json_field "$(api "$SOURCE_BASE" "$MEMBER_JAR" POST /documents "$REF_DOC_CREATE_BODY")" id)"
api "$SOURCE_BASE" "$MEMBER_JAR" PUT "/documents/${REF_DOC_ID}/body" "$(python3 -c '
import json, sys
def mention(entity, target, label):
    return {"type": "mention", "attrs": {"entity": entity, "id": target, "label": label}}
document, member_task, moved_task = sys.argv[1:4]
print(json.dumps({"contentJson": {"type": "doc", "content": [{"type": "paragraph", "content": [
    {"type": "text", "text": "멤버 참조 "}, mention("document", document, "백업 문서"),
    mention("task", member_task, "멤버 작업"), mention("task", moved_task, "이동 작업")]}]}}))
' "$DOCUMENT_ID" "$MEMBER_TASK_ID" "$MOVED_TASK_ID")" >/dev/null
MEMBER_RUN_ID="$(timer_run "$SOURCE_BASE" "$MEMBER_JAR" "$WORKSPACE_ID" "$MEMBER_TASK_ID")"
log_assert "member wiki document naming the shared document, member task and moved task; member timer run: ok"
VIEW_CONFIG='{"query":{"filters":{}},"groupBy":null,"dateBy":null}'
WIKI_COLLECTION_ID="$(json_field "$(api "$SOURCE_BASE" "$COOKIE_JAR" POST /collections '{"name":"팀 위키 모음","kind":"document","projectId":null}')" id)"
CHOICE_FIELD="$(api "$SOURCE_BASE" "$COOKIE_JAR" POST "/collections/${WIKI_COLLECTION_ID}/fields" '{"name":"검토 상태","type":"select","options":["초안","검토"]}')"
PERSON_FIELD="$(api "$SOURCE_BASE" "$COOKIE_JAR" POST "/collections/${WIKI_COLLECTION_ID}/fields" '{"name":"위키 담당","type":"user","options":[]}')"
# A scalar value (stored in collection_values; choice/person values are not).
TEXT_FIELD="$(api "$SOURCE_BASE" "$COOKIE_JAR" POST "/collections/${WIKI_COLLECTION_ID}/fields" '{"name":"위키 메모","type":"text","options":[]}')"
WIKI_TEXT="복원 확인 메모 🙂"
api "$SOURCE_BASE" "$COOKIE_JAR" POST "/collections/${WIKI_COLLECTION_ID}/items" "{\"documentId\":\"${DOCUMENT_ID}\"}" >/dev/null
# put_wiki_value FIELD VALUE: one value on the shared wiki document's item.
put_wiki_value() {
  local item
  item="$(api "$SOURCE_BASE" "$COOKIE_JAR" GET "/documents/${DOCUMENT_ID}/collection-item")"
  api "$SOURCE_BASE" "$COOKIE_JAR" PUT "/collections/${WIKI_COLLECTION_ID}/items/$(json_field "$item" item id)/values" \
    "{\"fieldId\":\"$(json_field "$1" id)\",\"expectedVersion\":$(json_field "$item" item version),\"expectedFieldVersion\":$(json_field "$1" version),\"value\":$2}" >/dev/null
}
put_wiki_value "$CHOICE_FIELD" "{\"options\":[\"$(json_field "$CHOICE_FIELD" options 1 id)\"]}"
put_wiki_value "$PERSON_FIELD" "{\"users\":[\"${MEMBER_ID}\"]}"
put_wiki_value "$TEXT_FIELD" "{\"text\":\"${WIKI_TEXT}\"}"
SHARED_VIEW_ID="$(json_field "$(api "$SOURCE_BASE" "$COOKIE_JAR" POST "/collections/${WIKI_COLLECTION_ID}/views" "{\"name\":\"팀 공유 보기\",\"type\":\"table\",\"visibility\":\"shared\",\"config\":${VIEW_CONFIG}}")" id)"
PRIVATE_VIEW_ID="$(json_field "$(api "$SOURCE_BASE" "$MEMBER_JAR" POST "/collections/${WIKI_COLLECTION_ID}/views" "{\"name\":\"내 비공개 보기\",\"type\":\"table\",\"visibility\":\"private\",\"config\":${VIEW_CONFIG}}")" id)"
log_assert "wiki collection with choice and person values, shared (owner) and private (member) views: ok"

# Isolated Zotero producer (fixture mode only): the source server stops; the
# db-tests fixture (synthetic upstream, mode 22) serves the ordinary router
# on the same source database through the app role and the same storage
# volume, inside the stack's own network; the owner logs in with the
# original password, connects and syncs. The fixture then stops and the
# product server (default reader, no upstream use) resumes.
if [[ -n "$ZOTERO_FIXTURE_RECIPE" ]]; then
  log_assert "isolated Zotero producer (source server stopped)"
  "${SOURCE_COMPOSE[@]}" stop server
  cat >"$ZF_ENV" <<ZFENV
DATABASE_APP_URL=postgres://fvoci_app:${APP_PASSWORD}@postgres:5432/fvoci
FVOCI_E2E_SERVER_BIN=/opt/fvoci/bin/fvoci-server
FVOCI_E2E_DIST=/opt/fvoci/static
FVOCI_E2E_ZOTERO_STORAGE_DIR=/data/storage
ZFENV
  coproc ZF { docker run --rm -i --name "$ZF_NAME" --network "${SOURCE_PROJECT}_default" --env-file "$ZF_ENV" \
    -v "${SOURCE_PROJECT}_storage:/data/storage" --entrypoint /opt/fvoci/bin/fvoci-e2e-fixture \
    "$ZF_IMAGE" zotero-readonly 2>"$ZF_STDERR"; }
  ZF_CHILD="$ZF_PID"
  # zf_send JSON: one fixture command; the reply line lands in ZF_REPLY.
  zf_send() {
    printf '%s\n' "$1" >&"${ZF[1]}"
    IFS= read -r -t 120 ZF_REPLY <&"${ZF[0]}" || { echo "zotero fixture gave no reply" >&2; return 1; }
  }
  # zf_api METHOD PATH [BODY]: an ordinary request from inside the fixture's
  # network namespace (cookie jar inside the container; body on stdin).
  zf_api() {
    if [[ -n "${3:-}" ]]; then
      printf '%s' "$3" | docker exec -i "$ZF_NAME" curl -fsS -c /tmp/zf-jar -b /tmp/zf-jar \
        -H "content-type: application/json" -H "origin: $ZF_ORIGIN" -X "$1" "$ZF_ORIGIN$2" --data-binary @-
    else
      docker exec "$ZF_NAME" curl -fsS -c /tmp/zf-jar -b /tmp/zf-jar -H "origin: $ZF_ORIGIN" -X "$1" "$ZF_ORIGIN$2"
    fi
  }
  IFS= read -r -t 180 ZF_REPLY <&"${ZF[0]}" || fail "zotero fixture did not start"
  ZF_ORIGIN="$(json_field "$ZF_REPLY" origin)"
  zf_api POST /api/v1/auth/login "{\"email\":\"${OWNER_EMAIL}\",\"password\":\"${OWNER_PASSWORD_LOGIN}\"}" >/dev/null
  zf_send '{"command":"mode","mode":22}'
  python3 -c 'import json,sys; assert json.loads(sys.argv[1]) == {"ok": True}, sys.argv[1]' "$ZF_REPLY"
  ZOTERO_CONNECTOR_ID="$(json_field "$(zf_api POST "/api/v1/workspaces/${PERSONAL_ID}/zotero" \
    "{\"libraryType\":\"user\",\"remoteLibraryId\":\"42\",\"apiKey\":\"${ZOTERO_FIXTURE_KEY}\",\"libraryUrl\":\"https://www.zotero.org/users/42\"}")" id)"
  python3 -c '
import json, sys
library = json.loads(sys.argv[1])
connector = library["connector"]
assert connector["state"] == "connected" and connector["generation"] == "1" and connector["completedVersion"] == "99", connector
assert len(library["references"]) == 1 and library["collections"], library
' "$(zf_api POST "/api/v1/workspaces/${PERSONAL_ID}/zotero/libraries/${ZOTERO_CONNECTOR_ID}/sync")"
  zf_send "{\"command\":\"observe\",\"userId\":\"${OWNER_ID}\",\"workspaceId\":\"${PERSONAL_ID}\",\"connectorId\":\"${ZOTERO_CONNECTOR_ID}\"}"
  python3 -c '
import json, sys
seen = json.loads(sys.argv[1])
assert seen["restrictedRole"] is True and seen["credentialRows"] == 1 and seen["connector"]["state"] == "connected", seen
assert len(seen["rows"]) == 1, seen["rows"]
' "$ZF_REPLY"
  zf_send '{"command":"requests"}'
  python3 -c 'import json,sys; assert json.loads(sys.argv[1])["requests"], "no synthetic upstream request"' "$ZF_REPLY"
  zf_send '{"command":"stop"}'
  python3 -c 'import json,sys; assert json.loads(sys.argv[1]) == {"stopped": True, "ownedResources": 0}, sys.argv[1]' "$ZF_REPLY"
  wait "$ZF_CHILD"
  SEALED_ZOTERO="$(poll_count "$SOURCE_PROJECT" "$SOURCE_ENV" "SELECT count(*) FROM fvoci.zotero_credentials WHERE sealed_key LIKE 'enc:v2:${ENC_ID}:%'")"
  [[ "$SEALED_ZOTERO" == "1" ]] || fail "expected one sealed Zotero credential, got ${SEALED_ZOTERO}"
  "${SOURCE_COMPOSE[@]}" start server
  wait_http "$SOURCE_BASE" "/api/v1/setup"
  log_assert "Zotero connect + mode 22 sync by the owner (app role, synthetic upstream only, sealed credential), fixture stopped, product server resumed: ok"
fi
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
python3 - "$SOURCE_OWNER_ORACLE" "$SOURCE_MEMBER_ORACLE" "$MOVED_DOC_ID" "$MOVED_TASK_ID" "$MOVED_ATTACHMENT_ID" \
  "$REF_DOC_ID" "$OWNER_RUN_ID" "$MEMBER_RUN_ID" "$SHARED_VIEW_ID" "$PRIVATE_VIEW_ID" "$ZOTERO_CONNECTOR_ID" \
  "$(json_field "$TEXT_FIELD" id)" "$WIKI_TEXT" <<'PY'
import json, sys
owner, member = json.loads(sys.argv[1]), json.loads(sys.argv[2])
moved_doc, moved_task, attachment, ref_doc, owner_run, member_run, shared, private, connector = sys.argv[3:12]
text_field, wiki_text = sys.argv[12:14]
def froms(listing):
    return {item["from"]["id"] for item in listing["items"]}
def runs(history):
    return {item.get("runId") for item in history["items"]}
for view in (owner, member):
    assert view["movedTask"]["id"] == moved_task and view["personalMovedTaskStatus"] in ("403", "404"), view["personalMovedTaskStatus"]
    body = json.dumps(view["movedBody"]["contentJson"])
    assert moved_task in body and attachment in body, "moved body lost its task mention or file"
    assert {moved_doc, ref_doc} <= froms(view["movedBacklinks"]), view["movedBacklinks"]
    assert ref_doc in froms(view["documentBacklinks"]) and ref_doc in froms(view["memberTaskBacklinks"])
    assert all(r["items"] for r in view["movedRevisions"]), "moved document/task revisions missing"
    assert len(view["wikiItems"]) == 1 and len(view["wikiFields"]["items"]) == 3, (view["wikiItems"], view["wikiFields"])
    assert view["wikiItems"][0]["values"][text_field] == {"text": wiki_text}, view["wikiItems"][0]["values"]
assert owner_run in runs(owner["movedTimerHistory"]) and owner_run not in runs(member["movedTimerHistory"])
assert member_run in runs(member["memberTaskTimerHistory"]) and member_run not in runs(owner["memberTaskTimerHistory"])
owner_views = {v["id"] for v in owner["wikiViews"]["items"]}
member_views = {v["id"] for v in member["wikiViews"]["items"]}
assert shared in owner_views and private not in owner_views and {shared, private} <= member_views, (owner_views, member_views)
assert member["zotero"] in ("403", "404"), member["zotero"]
if connector:
    listing, library = owner["zotero"]
    assert [c["id"] for c in listing["connectors"]] == [connector] and library["connector"]["state"] == "connected", owner["zotero"]
    assert len(library["references"]) == 1, library
else:
    assert owner["zotero"]["connectors"] == [], owner["zotero"]
PY
log_assert "per-user source model reads (moved ids/body/file/history/backlinks, own timer only, wiki values and view privacy, Zotero mirror owner-only): ok"

# A secret sealed with ENCRYPTION_KEYS (MFA setup stores the TOTP secret
# sealed; it is not enabled, so password login stays single-factor).
curl -fsS -b "$COOKIE_JAR" -H "content-type: application/json" -H "origin: $SOURCE_BASE" \
  -X POST "$SOURCE_BASE/api/v1/auth/mfa/setup" \
  -d "{\"currentPassword\":\"${OWNER_PASSWORD_LOGIN}\"}" >/dev/null
SEALED_MFA="$(poll_count "$SOURCE_PROJECT" "$SOURCE_ENV" "SELECT count(*) FROM fvoci.user_mfa WHERE totp_secret LIKE 'enc:v2:${ENC_ID}:%'")"
if [[ "$SEALED_MFA" != "1" ]]; then
  fail "expected one sealed MFA secret, got ${SEALED_MFA}"
fi
log_assert "sealed MFA secret on source: ok"

phase backup
log_assert "backup source stack"
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
  fail "backup permissions expected dir 700 files 600, got dir=${MODE_DIR} dump=${MODE_DUMP} tar=${MODE_TAR} manifest=${MODE_MANIFEST}"
fi
ENC_K1="$ENC_K1" ENC_ID="$ENC_ID" python3 - "$BACKUP_DIR/manifest.json" <<'PY'
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
assert sorted(keys["keyFingerprints"]) == [os.environ["ENC_ID"]], keys
assert os.environ["ENC_K1"] not in blob, "raw ENCRYPTION_KEYS key in manifest"
PY
log_assert "backup archive private + no extra secrets, search omitted: ok ($((SECONDS - BACKUP_START))s)"
# The server stays stopped after the dump: these rows are the backed-up state.
SOURCE_TEAM_FINGERPRINT="$(team_fingerprint "$SOURCE_PROJECT" "$SOURCE_ENV")"
if [[ -z "$SOURCE_TEAM_FINGERPRINT" ]]; then
  fail "source team fingerprint is empty"
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
MODEL_COUNT_SQL=""
for table in "${MODEL_TABLES[@]}"; do
  MODEL_COUNT_SQL+="SELECT '${table}='||count(*) FROM fvoci.${table} UNION ALL "
done
SOURCE_MODEL_COUNTS="$(poll_count "$SOURCE_PROJECT" "$SOURCE_ENV" "SELECT string_agg(x, ';' ORDER BY x) FROM (${MODEL_COUNT_SQL% UNION ALL }) AS q(x)")"
python3 - "$SOURCE_MODEL_COUNTS" "${ZOTERO_CONNECTOR_ID:+zotero}" <<'PY'
import sys
counts = {k: int(v) for k, v in (item.split("=") for item in sys.argv[1].split(";"))}
nonempty = ["documents", "personal_transfer_commands", "task_origins", "time_entries", "task_timer_runs",
            "task_timer_segments", "task_timer_commands", "task_timer_audit", "collections", "collection_items",
            "collection_options", "collection_choices", "collection_values", "collection_views"]
if sys.argv[2]:
    nonempty += ["zotero_connectors", "zotero_credentials", "zotero_references", "zotero_collections"]
assert all(counts[t] >= 1 for t in nonempty), counts
assert counts["collection_views"] >= 2 and counts["task_timer_runs"] >= 2, counts
PY
SOURCE_MODEL_FINGERPRINT="$(team_fingerprint "$SOURCE_PROJECT" "$SOURCE_ENV" "${MODEL_TABLES[@]}")"
log_assert "source current-model rows (${SOURCE_MODEL_COUNTS}) fingerprint taken (not printed): ok"

phase restore-refusals
log_assert "destroy source stack and volumes"
"${SOURCE_COMPOSE[@]}" down -v --remove-orphans
if docker volume inspect "${SOURCE_PROJECT}_storage" >/dev/null 2>&1; then
  fail "source storage volume still exists after down -v"
fi
log_assert "source stack and volumes removed: ok"

RESTORE_PORT="$(pick_port)"
RESTORE_BASE="http://127.0.0.1:${RESTORE_PORT}"
log_assert "restore with a different pepper must be refused"
WRONG_ENV="$(mktemp "${TMPDIR:-/tmp}/fvoci-br-wrong-env.${RUN_ID}.XXXXXX")"
chmod 600 "$WRONG_ENV"
sed -E "s#^PASSWORD_PEPPER_KEYS=.*#PASSWORD_PEPPER_KEYS={\"${PEPPER_ID}\":\"$(openssl rand -hex 32)\"}#" "$SOURCE_ENV" >"$WRONG_ENV"
WRONG_PROJECT="${RESTORE_PROJECT}-wrongpepper"
if bash "$ROOT/scripts/restore.sh" --project "$WRONG_PROJECT" --env-file "$WRONG_ENV" --input "$BACKUP_DIR" >/dev/null 2>&1; then
  rm -f "$WRONG_ENV"
  fail "restore with a different pepper keyring must fail"
fi
rm -f "$WRONG_ENV"
if docker volume ls --format '{{.Name}}' | grep -q "^${WRONG_PROJECT}_"; then
  fail "refused restore must not create volumes"
fi
log_assert "restore with a different pepper refused before touching anything: ok"

log_assert "restore with a different key under a backed-up ENCRYPTION_KEYS id must be refused"
WRONG_ENV="$(mktemp "${TMPDIR:-/tmp}/fvoci-br-wrong-env.${RUN_ID}.XXXXXX")"
chmod 600 "$WRONG_ENV"
sed -E "s#^ENCRYPTION_KEYS=.*#ENCRYPTION_KEYS={\"${ENC_ID}\":\"$(openssl rand -hex 32)\"}#" "$SOURCE_ENV" >"$WRONG_ENV"
WRONG_PROJECT="${RESTORE_PROJECT}-wrongkeys"
if bash "$ROOT/scripts/restore.sh" --project "$WRONG_PROJECT" --env-file "$WRONG_ENV" --input "$BACKUP_DIR" >/dev/null 2>&1; then
  rm -f "$WRONG_ENV"
  fail "restore with a different ENCRYPTION_KEYS key must fail"
fi
rm -f "$WRONG_ENV"
if docker volume ls --format '{{.Name}}' | grep -q "^${WRONG_PROJECT}_"; then
  fail "refused restore must not create volumes"
fi
log_assert "restore with a mis-keyed ENCRYPTION_KEYS refused before touching anything: ok"

write_env "$RESTORE_ENV" "$RESTORE_OWNER_PASSWORD" "$RESTORE_APP_PASSWORD" \
  "$RESTORE_MEILI_MASTER_KEY" "$RESTORE_BASE" "$RESTORE_PORT" \
  "$RESTORE_ENCRYPTION_KEYS" k2

phase restore
log_assert "restore into a fresh project"
RESTORE_STARTED=1
RESTORE_START=$SECONDS
RESTORE_OUT="$(bash "$ROOT/scripts/restore.sh" \
  --project "$RESTORE_PROJECT" \
  --env-file "$RESTORE_ENV" \
  --input "$BACKUP_DIR")"
printf '%s\n' "$RESTORE_OUT"
python3 -c 'import json,sys; body=json.loads(sys.argv[1].strip().splitlines()[-1]); assert body.get("secretsVerified") is True, body' "$RESTORE_OUT"
if grep -Fq "$ENC_K1" <<<"$RESTORE_OUT"; then
  fail "restore output printed an ENCRYPTION_KEYS key"
fi
log_assert "restore compose up with a rotated superset keyring, secrets opened: ok ($((SECONDS - RESTORE_START))s) base=${RESTORE_BASE}"
if [[ "$(team_fingerprint "$RESTORE_PROJECT" "$RESTORE_ENV")" != "$SOURCE_TEAM_FINGERPRINT" ]]; then
  fail "restored team rows differ from the backed-up rows (fingerprint mismatch)"
fi
log_assert "restored team metadata equals the backed-up rows: ok"
if [[ "$(native_fingerprint "$RESTORE_PROJECT" "$RESTORE_ENV")" != "$SOURCE_NATIVE_FINGERPRINT" ]]; then
  fail "restored native history / attachment metadata differ from the backed-up rows"
fi
log_assert "restored native history and immutable attachment metadata equal the backed-up rows: ok"
if [[ "$(team_fingerprint "$RESTORE_PROJECT" "$RESTORE_ENV" "${MODEL_TABLES[@]}")" != "$SOURCE_MODEL_FINGERPRINT" ]]; then
  fail "restored current-model rows differ from the backed-up rows"
fi
log_assert "restored current-model rows (moved graph, timers, wiki collection, Zotero mirror) equal the backed-up rows: ok"

phase verify-restore
expect_service_image "$RESTORE_PROJECT" server "$IMAGE_ID"
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
  fail "restored download sha256 mismatch: $DOWNLOAD_SHA != $FIXTURE_SHA"
fi
log_assert "restored attachment sha256 matches: ok"

EXTRACT_STATUS="$(poll_extract_field "$RESTORE_PROJECT" "$RESTORE_ENV" extract_status "$ATTACHMENT_ID")"
EXTRACT_TEXT="$(poll_extract_field "$RESTORE_PROJECT" "$RESTORE_ENV" extract_text "$ATTACHMENT_ID")"
if [[ "$EXTRACT_STATUS" != "ok" ]] || ! grep -q '안녕' <<<"$EXTRACT_TEXT"; then
  fail "restored extraction lost: $EXTRACT_STATUS $EXTRACT_TEXT"
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
  fail "restored member login returned another user"
fi
log_assert "second member login with original password after restore: ok"
if [[ "$(user_oracle "$RESTORE_BASE" "$COOKIE_JAR")" != "$SOURCE_OWNER_ORACLE" ]]; then
  fail "restored owner reads differ from the source"
fi
if [[ "$(user_oracle "$RESTORE_BASE" "$MEMBER_JAR")" != "$SOURCE_MEMBER_ORACLE" ]]; then
  fail "restored member reads differ from the source"
fi
log_assert "per-user restored reads equal the source (grants, HID 404, history, comments, revisions incl. member task/wiki revision detail, person value, moved graph, own timers, wiki values/views, backlinks, Zotero mirror): ok"

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
  fail "restored task is not searchable after rebuild: ${SEARCH_JSON}"
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
  fail "restored team tasks are not searchable after rebuild"
fi
if search_has "$MEMBER_JAR" "Hidden%20owner%20task" "$HID_TASK_ID"; then
  fail "restored search shows the owner's hidden task to the member"
fi
log_assert "restored search is per-person (member PRV yes, HID no; owner HID yes): ok"

RESTORE_CID="$("${RESTORE_COMPOSE[@]}" ps -q server)"
RUNNING_UID="$(docker exec "$RESTORE_CID" id -u)"
SERVER_PID1_UID="$(docker exec "$RESTORE_CID" stat -c '%u' /proc/1)"
if [[ "$RUNNING_UID" != "1000" || "$SERVER_PID1_UID" != "1000" ]]; then
  fail "restored server must run as uid 1000, got exec=${RUNNING_UID} pid1=${SERVER_PID1_UID}"
fi
log_assert "restored server runs as non-root uid 1000: ok"
SERVER_ENV="$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$RESTORE_CID")"
if grep -Eq '^(DATABASE_URL|FVOCI_MIGRATION_URL)=' <<<"$SERVER_ENV"; then
  fail "restored server must not receive the migration owner URL"
fi
if ! grep -Eq '^DATABASE_APP_URL=' <<<"$SERVER_ENV"; then
  fail "restored server missing DATABASE_APP_URL"
fi
log_assert "restored server holds only the app database URL: ok"
if grep -Eq '^(MEILI_MASTER_KEY|FVOCI_MEILI_MASTER_KEY)=' <<<"$SERVER_ENV"; then
  fail "restored server must not receive the Meilisearch master key"
fi
log_assert "restored server does not hold the Meili master key: ok"

phase collect
TOTAL=$((SECONDS - START_TS))
log_assert "backup-restore-smoke complete (${TOTAL}s)"
cat "$ASSERT_LOG"
