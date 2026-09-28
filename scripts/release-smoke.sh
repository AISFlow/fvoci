#!/usr/bin/env bash
# Release smoke for one architecture (docs/RELEASING.md). Runs only against the
# image index recorded in a rendered release directory, which the workflow
# pushed BY DIGEST: no :0.y.z or :0.y tag exists until both smokes passed.
#   - anonymous registry access (empty DOCKER_CONFIG, no login) and a pull by
#     digest onto a daemon that did not hold the image before;
#   - the user procedure with the rendered files: compose.yml (the user
#     compose's x-fvoci-image anchor pinned to the digest) and env.example
#     copied to .env with a fresh value generated for every empty entry, as its
#     comments show, in an empty directory with a scrubbed environment;
#   - health/ready, doctor, the server process running as uid 1000 and holding
#     neither the database owner password nor the Meilisearch master key, the
#     secret files root-only, no secret in docker exec environ or inspect, first admin setup and
#     login, the install-smoke API flows (collab, documents, attachment +
#     extraction), search, keys kept across a second up and down/up, a failing
#     preparation keeping the server down, an unfilled .env refused before any
#     container starts, a placeholder value refused by the app, and a browser
#     subset against fresh stacks.
# Service names are not assumed: the app is the service publishing port 8080.
#
#   scripts/release-smoke.sh --dist DIR [--no-browser]
# The browser subset needs apps/web dependencies and Playwright Chromium.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST=""
BROWSER=1
while (($#)); do
  case "$1" in
    --dist) DIST="$(cd "${2:?--dist needs a directory}" && pwd)"; shift 2 ;;
    --no-browser) BROWSER=0; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[[ -n "$DIST" ]] || { echo "--dist is required" >&2; exit 2; }

# Specs that create their own first admin through /setup and need no DB
# fixture binary, SMTP capture or server restart hook; each gets a fresh stack.
BROWSER_SPECS=(
  task-edit-flow.spec.ts                  # setup/login, workspace, project, task
  project-document-revisions-flow.spec.ts # collaborative document edit, revision restore
  task-attachments-flow.spec.ts           # attachment upload, preview, download
  attachment-hwp-edit-flow.spec.ts        # HWPX edit, draft download, save-copy
)

for cmd in docker curl python3 node openssl sha256sum; do
  command -v "$cmd" >/dev/null 2>&1 || { echo "missing required command: $cmd" >&2; exit 1; }
done

RUN_ID="$(openssl rand -hex 6)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-release-smoke.XXXXXX")"
ASSERT_LOG="$WORK/assertions.log"
: >"$ASSERT_LOG"
PROJECTS=()
START_TS=$SECONDS

log_assert() { printf '%s\n' "$1" | tee -a "$ASSERT_LOG"; }
fail() { echo "release-smoke: $*" >&2; exit 1; }

# No credential store, no helpers: every registry call below is anonymous.
export DOCKER_CONFIG="$WORK/docker-config"
mkdir -p "$DOCKER_CONFIG"
printf '{}\n' >"$DOCKER_CONFIG/config.json"

# Compose sees only what a user shell would need; nothing from the runner leaks
# into interpolation.
compose_in() {
  local dir="$1" project="$2"
  shift 2
  (cd "$dir" && env -i PATH="$PATH" HOME="$HOME" DOCKER_CONFIG="$DOCKER_CONFIG" \
    docker compose --project-name "$project" "$@")
}

cleanup() {
  local status=$?
  local entry
  for entry in "${PROJECTS[@]}"; do
    if (( status != 0 )); then
      echo "== ${entry#*|} logs (last 200 lines)" >&2
      compose_in "${entry%%|*}" "${entry#*|}" logs --no-color --tail 200 >&2 || true
    fi
    compose_in "${entry%%|*}" "${entry#*|}" down -v --remove-orphans >/dev/null 2>&1 || true
  done
  if (( status != 0 )); then
    echo "release-smoke failed after $((SECONDS - START_TS))s; assertions:" >&2
    cat "$ASSERT_LOG" >&2 || true
  fi
  rm -rf "$WORK"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

(cd "$DIST" && sha256sum --strict -c SHA256SUMS >/dev/null) || fail "SHA256SUMS does not match $DIST"
# json_get JSON KEY... prints the value at that path (numeric keys index lists).
json_get() {
  python3 -c 'import json, sys
value = json.loads(sys.argv[1])
for key in sys.argv[2:]:
    value = value[int(key) if key.isdigit() else key]
print(value)' "$@"
}
RECORD="$(cat "$DIST/release.json")"
VERSION="$(json_get "$RECORD" version)"
SOURCE_SHA="$(json_get "$RECORD" sourceSha)"
IMAGE_REF="$(json_get "$RECORD" image)"
INDEX_DIGEST="$(json_get "$RECORD" indexDigest)"
REPO="${IMAGE_REF%%:*}"
case "$(uname -m)" in
  x86_64) ARCH=amd64 ;;
  aarch64) ARCH=arm64 ;;
  *) fail "unsupported runner architecture $(uname -m)" ;;
esac
ARCH_DIGEST="$(json_get "$RECORD" platforms "linux/$ARCH")"
log_assert "== release ${VERSION} (${SOURCE_SHA}) on linux/${ARCH}: ${IMAGE_REF}"

if docker image inspect "$IMAGE_REF" >/dev/null 2>&1 || docker image inspect "$REPO@$ARCH_DIGEST" >/dev/null 2>&1; then
  fail "the daemon already holds $REPO; the smoke must pull the published digest onto a clean daemon"
fi

if ! INDEX_JSON="$(docker manifest inspect "$REPO@$INDEX_DIGEST" 2>"$WORK/manifest.err")"; then
  cat "$WORK/manifest.err" >&2
  echo "::error::Anonymous 'docker manifest inspect $REPO@$INDEX_DIGEST' failed. GHCR packages of an organization start private: make the package public (docs/RELEASING.md) and re-run the failed smoke jobs." >&2
  exit 1
fi
python3 - "$INDEX_JSON" "$DIST/release.json" <<'PY'
import json, sys
index = json.loads(sys.argv[1])
record = json.load(open(sys.argv[2]))
found = {
    f"{m['platform']['os']}/{m['platform']['architecture']}": m["digest"]
    for m in index.get("manifests", [])
    if m.get("platform", {}).get("os") != "unknown"
}
assert found == record["platforms"], (found, record["platforms"])
PY
log_assert "anonymous manifest inspect of the published index; per-arch digests match the release record: ok"

# By digest only: the version tag is applied after this smoke passes.
DIGEST_REF="$REPO@$INDEX_DIGEST"
docker pull --quiet "$DIGEST_REF" >/dev/null
LABELS="$(docker image inspect -f '{{json .Config.Labels}}' "$DIGEST_REF")"
PULLED="$(docker image inspect -f '{{json .RepoDigests}} {{.Os}}/{{.Architecture}}' "$DIGEST_REF")"
python3 - "$LABELS" "$VERSION" "$SOURCE_SHA" <<'PY'
import json, sys
labels, version, sha = json.loads(sys.argv[1]) or {}, sys.argv[2], sys.argv[3]
assert labels.get("org.opencontainers.image.version") == version, labels
assert labels.get("org.opencontainers.image.revision") == sha, labels
PY
[[ "$PULLED" == *"$REPO@$INDEX_DIGEST"* && "$PULLED" == *" linux/$ARCH" ]] || fail "pulled image does not match: $PULLED"
log_assert "pulled by digest; OCI version/revision labels match ${VERSION}/${SOURCE_SHA}: ok"
# The release build passes FVOCI_BUILD_SHA; the Dockerfile must forward it
# (ARG in the Rust build stage) or this reports "unknown".
BUILD_ID="$(docker run --rm "$DIGEST_REF" --version)"
[[ "$BUILD_ID" == "fvoci-server ${VERSION} (${SOURCE_SHA})" ]] || fail "fvoci-server --version reports '$BUILD_ID'"
log_assert "fvoci-server --version: ${BUILD_ID}: ok"

docker run --rm --entrypoint sh "$DIGEST_REF" -ec '
  for runtime in node nodejs bun deno qjs quickjs js d8 jsc; do
    if command -v "$runtime" >/dev/null 2>&1; then echo "unexpected script runtime: $runtime" >&2; exit 1; fi
  done
  for program in fvoci-server fvoci-migrate collab-engine document-extract; do test -x "/opt/fvoci/bin/$program"; done
'
log_assert "product executables present, no JavaScript runtime in the image: ok"

# --- stacks -----------------------------------------------------------------

# fill_env EXAMPLE OUT: every empty entry gets a fresh value in the format its
# comment shows (openssl rand -hex 32; a *_KEYS keyring under its
# *_ACTIVE_KEY_ID). Filled entries are kept.
fill_env() {
  python3 - "$1" "$2" <<'PY'
import re, secrets, sys
text = open(sys.argv[1], encoding="utf-8").read()
values = dict(re.findall(r"^([A-Z][A-Z0-9_]*)=(.*)$", text, re.MULTILINE))
out = []
for line in text.splitlines():
    empty = re.fullmatch(r"([A-Z][A-Z0-9_]*)=", line)
    if empty:
        key = empty.group(1)
        if key.endswith("_KEYS"):
            active = values.get(key[: -len("_KEYS")] + "_ACTIVE_KEY_ID") or sys.exit(f"no active key id for {key}")
            line = f'{key}={{"{active}":"{secrets.token_hex(32)}"}}'
        else:
            line = f"{key}={secrets.token_hex(32)}"
    out.append(line)
open(sys.argv[2], "w", encoding="utf-8").write("\n".join(out) + "\n")
PY
  chmod 600 "$2"
}

new_stack() { # name [unfilled] -> sets STACK_DIR STACK_PROJECT
  STACK_DIR="$WORK/$1"
  STACK_PROJECT="fvoci-release-$1-${RUN_ID}"
  mkdir -p "$STACK_DIR"
  cp "$DIST/compose.yml" "$STACK_DIR/compose.yml"
  if [[ "${2:-}" == unfilled ]]; then
    cp "$DIST/env.example" "$STACK_DIR/.env"
  else
    fill_env "$DIST/env.example" "$STACK_DIR/.env"
  fi
  PROJECTS+=("$STACK_DIR|$STACK_PROJECT")
}
set_env() { # KEY VALUE in the current stack's .env
  python3 - "$STACK_DIR/.env" "$1" "$2" <<'PY'
import re, sys
path, key, value = sys.argv[1:]
text = open(path, encoding="utf-8").read()
text, n = re.subn(rf"^{key}=.*$", lambda _: f"{key}={value}", text, flags=re.MULTILINE)
assert n == 1, key
open(path, "w", encoding="utf-8").write(text)
PY
}
dc() { compose_in "$STACK_DIR" "$STACK_PROJECT" "$@"; }

wait_http() {
  local url="$1" deadline=$((SECONDS + 90))
  while (( SECONDS < deadline )); do
    curl -fsS "$url" >/dev/null 2>&1 && return 0
    sleep 0.5
  done
  fail "timed out waiting for $url"
}

# The app service: the one publishing container port 8080.
app_service() {
  dc config --format json | python3 -c '
import json, sys
services = json.load(sys.stdin)["services"]
apps = [n for n, s in services.items() if any(p.get("target") == 8080 for p in s.get("ports") or [])]
if len(apps) != 1:
    sys.exit(f"expected one service publishing 8080, found {apps}")
print(apps[0])'
}

# The address a user opens: the server's configured public origin when set,
# else the published port.
resolve_base_url() {
  local cid published origin
  cid="$(dc ps -q "$APP")"
  [[ -n "$cid" ]] || fail "$APP container missing"
  origin="$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$cid" | sed -n 's/^FVOCI_PUBLIC_ORIGIN=//p')"
  published="$(dc port "$APP" 8080 | head -1)"
  [[ -n "$published" ]] || fail "$APP port 8080 is not published"
  BASE_URL="${origin:-http://127.0.0.1:${published##*:}}"
  BASE_URL="${BASE_URL%/}"
  SERVER_CID="$cid"
}

start_stack() {
  dc up -d --wait
  resolve_base_url
  wait_http "$BASE_URL/health"
}

db_query() {
  # shellcheck disable=SC2016 # expanded by the postgres container's shell
  dc exec -T postgres sh -c 'psql -U "$POSTGRES_USER" -d "${POSTGRES_DB:-$POSTGRES_USER}" -tAc "$1"' sh "$1" | tr -d '[:space:]'
}

# --- install from the rendered compose --------------------------------------

new_stack install
[[ "$(find "$STACK_DIR" -mindepth 1 -printf '%f\n' | sort | tr '\n' ' ')" == ".env compose.yml " ]] || fail "the install directory holds more than compose.yml and .env"
APP="$(app_service)" || fail "no single app service"
dc config --format json | python3 -c '
import json, sys
services = json.load(sys.stdin)["services"]
product = sorted(n for n, s in services.items() if s.get("image") == sys.argv[1])
assert sys.argv[2] in product, (sys.argv[2], product)
assert not any(s.get("env_file") for s in services.values())
print("product image services:", " ".join(product), "app:", sys.argv[2])' "$IMAGE_REF" "$APP" | tee -a "$ASSERT_LOG"
log_assert "== docker compose up -d --wait with compose.yml and the filled env.example"
UP_START=$SECONDS
start_stack
log_assert "compose up: ok ($((SECONDS - UP_START))s) base=${BASE_URL}"

for probe in /health /ready; do
  [[ "$(curl -sS -o /dev/null -w '%{http_code}' "$BASE_URL$probe")" == 200 ]] || fail "$probe is not 200"
done
log_assert "/health and /ready 200: ok"

if ! DOCTOR_REPORT="$(dc exec -T "$APP" /opt/fvoci/bin/fvoci-migrate --doctor)"; then
  printf '%s\n' "$DOCTOR_REPORT" >&2
  fail "doctor failed"
fi
python3 -c 'import json,sys; r=json.load(sys.stdin); assert r["ok"] is True, r' <<<"$DOCTOR_REPORT"
log_assert "installed doctor: ok"

[[ "$(docker exec --user 1000:1000 "$SERVER_CID" readlink /proc/1/exe)" == /opt/fvoci/bin/fvoci-server ]] \
  || fail "pid 1 of $APP is not fvoci-server"
# uid boundary: the app container starts as root, reads the root-only secret
# files, prepares, and runs the server as uid/gid 1000 without capabilities.
PID1="$(docker exec "$SERVER_CID" sh -c 'grep -E "^(Uid|Gid|Groups|CapPrm|CapEff):" /proc/1/status')"
grep -Eq '^Uid:[[:space:]]+1000[[:space:]]+1000[[:space:]]+1000[[:space:]]+1000$' <<<"$PID1" || fail "server uids are not all 1000"
grep -Eq '^Gid:[[:space:]]+1000[[:space:]]+1000[[:space:]]+1000[[:space:]]+1000$' <<<"$PID1" || fail "server gids are not all 1000"
grep -Eq '^Groups:[[:space:]]*$' <<<"$PID1" || fail "server keeps supplementary groups"
grep -Eq '^CapEff:[[:space:]]+0+$' <<<"$PID1" || fail "server keeps effective capabilities"
grep -Eq '^CapPrm:[[:space:]]+0+$' <<<"$PID1" || fail "server keeps permitted capabilities"
env_value() { sed -n "s/^$1=//p" "$STACK_DIR/.env"; }
OWNER_PW="$(env_value POSTGRES_PASSWORD)"
MASTER_KEY="$(env_value MEILI_MASTER_KEY)"
APP_PW="$(env_value FVOCI_APP_PASSWORD)"
(( ${#OWNER_PW} >= 16 && ${#MASTER_KEY} >= 16 && ${#APP_PW} >= 16 )) || fail "could not read the generated secrets from .env"
for f in postgres_password fvoci_app_password meili_master_key; do
  [[ "$(docker exec "$SERVER_CID" stat -c '%u %g %a' "/run/secrets/$f")" == "0 0 400" ]] || fail "/run/secrets/$f is not root-only 0400"
  if docker exec --user 1000:1000 "$SERVER_CID" cat "/run/secrets/$f" >/dev/null 2>&1; then
    fail "uid 1000 can read /run/secrets/$f"
  fi
done
# The server process tree (pid 1 and its descriptors, children, argv and files
# under /run other than the secrets; read as uid 1000, since root in the
# container lacks CAP_SYS_PTRACE) holds neither the owner password nor the
# master key; the container configuration, and so every docker exec and
# healthcheck process, holds none of the three.
# shellcheck disable=SC2016 # expanded by the app container's shell
SERVER_VIEW="$(docker exec --user 1000:1000 "$SERVER_CID" sh -c '
  for p in /proc/[0-9]*; do
    a=${p#/proc/}
    while [ "$a" != 1 ] && [ "$a" != 0 ] && [ -n "$a" ]; do a=$(sed -n "s/^PPid:[[:space:]]*//p" "/proc/$a/status" 2>/dev/null); done
    [ "$a" = 1 ] || continue
    tr "\0" "\n" <"$p/environ"; tr "\0" " " <"$p/cmdline"; echo; ls -l "$p/fd"
  done 2>/dev/null
  find /run -path /run/secrets -prune -o -type f -exec cat {} + 2>/dev/null; echo')"
if grep -qF -e "$OWNER_PW" -e "$MASTER_KEY" <<<"$SERVER_VIEW"; then
  fail "the server process tree holds the database owner password or the Meilisearch master key"
fi
if grep -Eq '^(POSTGRES_PASSWORD|DATABASE_URL|FVOCI_MIGRATION_URL|MEILI_MASTER_KEY|FVOCI_MEILI_MASTER_KEY|FVOCI_APP_PASSWORD)(_FILE)?=' <<<"$SERVER_VIEW"; then
  fail "a preparation-only variable reached the server process tree"
fi
# shellcheck disable=SC2016 # expanded by the app container's shell
EXEC_ENV="$(docker exec "$SERVER_CID" sh -c 'tr "\0" "\n" </proc/self/environ')"
if grep -qF -e "$OWNER_PW" -e "$MASTER_KEY" -e "$APP_PW" <<<"$EXEC_ENV$(docker inspect "$SERVER_CID")"; then
  fail "docker exec / healthcheck environment or docker inspect holds a secret"
fi
log_assert "server is pid 1 fvoci-server as uid/gid 1000 without capabilities; /run/secrets root-only; owner password and master key absent from its process tree and /run; no secret in docker exec environ or docker inspect: ok"

ORIGIN="$BASE_URL"
COOKIE_JAR="$WORK/cookies"
: >"$COOKIE_JAR"
FIXTURE_HWPX="$ROOT/compat/fixtures/sample.hwpx"
DOCUMENT_STATE="$WORK/documents.json"
EMAIL="owner@release.test"
PASSWORD="release-$(openssl rand -hex 8)"
api() { curl -fsS -b "$COOKIE_JAR" -c "$COOKIE_JAR" -H "origin: $ORIGIN" "$@"; }

[[ "$(curl -fsS "$BASE_URL/")" == *"<"* ]] || fail "web shell not served"
api -H "content-type: application/json" -X POST "$BASE_URL/api/v1/setup" \
  -d "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\",\"givenName\":\"Owner\",\"workspaceSlug\":\"release\",\"workspaceName\":\"Release\"}" >/dev/null
SESSION="$(awk '$6 == "fvoci_session" { print $7; exit }' "$COOKIE_JAR")"
[[ -n "$SESSION" ]] || fail "setup did not return fvoci_session"
SECOND_SETUP="$(curl -sS -o /dev/null -w '%{http_code}' -H "content-type: application/json" -H "origin: $ORIGIN" \
  -X POST "$BASE_URL/api/v1/setup" -d '{"email":"x@release.test","password":"another-pass-1","givenName":"X","workspaceSlug":"x","workspaceName":"X"}')"
[[ "$SECOND_SETUP" =~ ^4 ]] || fail "second setup was not refused (HTTP $SECOND_SETUP)"
log_assert "first-admin setup, second setup refused (HTTP ${SECOND_SETUP}): ok"

login() {
  : >"$COOKIE_JAR"
  api -H "content-type: application/json" -X POST "$BASE_URL/api/v1/auth/login" \
    -d "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\"}" >/dev/null
  SESSION="$(awk '$6 == "fvoci_session" { print $7; exit }' "$COOKIE_JAR")"
  [[ -n "$SESSION" ]] || fail "login did not return fvoci_session"
}
login
log_assert "password login: ok"

WORKSPACE_ID="$(json_get "$(api "$BASE_URL/api/v1/me/workspaces")" items 0 id)"
DOC_TITLE="Release smoke ${RUN_ID}"
DOCUMENT_ID="$(json_get "$(api -H "content-type: application/json" -X POST \
  "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/documents" -d "{\"parentId\":null,\"title\":\"$DOC_TITLE\"}")" id)"
BODY_JSON="$(node "$ROOT/scripts/install-smoke-collab.mjs" --base-url "$BASE_URL" --origin "$ORIGIN" \
  --session "$SESSION" --workspace-id "$WORKSPACE_ID" --document-id "$DOCUMENT_ID")"
grep -q '"contentJson"' <<<"$BODY_JSON" || fail "collab body projection failed: $BODY_JSON"
log_assert "workspace document create + collab save and projection: ok"

python3 "$ROOT/scripts/install-smoke-documents.py" "$BASE_URL" "$WORKSPACE_ID" "$COOKIE_JAR" "$DOCUMENT_STATE" create
log_assert "document import/edit/exports/public PDF: ok"

FIXTURE_SHA="$(sha256sum "$FIXTURE_HWPX" | awk '{print $1}')"
UPLOAD_INIT="$(api -H "content-type: application/json" -X POST \
  "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/uploads" \
  -d "{\"name\":\"sample.hwpx\",\"sizeBytes\":$(wc -c <"$FIXTURE_HWPX"),\"declaredMime\":\"application/x-hwp\"}")"
ATTACHMENT_ID="$(json_get "$UPLOAD_INIT" attachmentId)"
PART_URL="$(json_get "$UPLOAD_INIT" parts 0 url)"
ETAG="$(api -X PUT "$BASE_URL${PART_URL}" --data-binary @"$FIXTURE_HWPX" -D - -o /dev/null \
  | awk 'tolower($1) == "etag:" { print $2; exit }' | tr -d '\r')"
api -H "content-type: application/json" -X POST \
  "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/attachments/${ATTACHMENT_ID}/complete" \
  -d "{\"parts\":[{\"partNumber\":1,\"etag\":\"${ETAG}\"}]}" >/dev/null

check_attachment() {
  local deadline=$((SECONDS + 90)) status="pending"
  while (( SECONDS < deadline )); do
    status="$(db_query "SELECT extract_status FROM fvoci.attachments WHERE id='${ATTACHMENT_ID}'")"
    [[ "$status" != "pending" ]] && break
    sleep 1
  done
  [[ "$status" == "ok" ]] || fail "extraction status: $status"
  db_query "SELECT extract_text FROM fvoci.attachments WHERE id='${ATTACHMENT_ID}'" | grep -q '안녕' \
    || fail "extracted text lost"
  api "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/attachments/${ATTACHMENT_ID}/download" -o "$WORK/download"
  [[ "$(sha256sum "$WORK/download" | awk '{print $1}')" == "$FIXTURE_SHA" ]] || fail "download bytes differ"
}
check_attachment
log_assert "attachment upload, extraction and byte-exact download: ok"

check_search() {
  local deadline=$((SECONDS + 60)) query
  query="$(python3 -c 'import sys,urllib.parse; print(urllib.parse.quote(sys.argv[1]))' "$DOC_TITLE")"
  while (( SECONDS < deadline )); do
    if api "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/search?q=${query}" \
      | python3 -c 'import json,sys; items=json.load(sys.stdin)["items"]; sys.exit(0 if any(sys.argv[1] in (i.get("documentId"), i.get("id")) for i in items) else 1)' "$DOCUMENT_ID"; then
      return 0
    fi
    sleep 1
  done
  fail "search did not find the document"
}
check_search
log_assert "workspace search finds the document: ok"

# --- restart persistence ------------------------------------------------------

key_check() { # the session and password login use the pepper; sealed secrets open
  [[ "$(curl -sS -o /dev/null -w '%{http_code}' -H "cookie: fvoci_session=$OLD_SESSION" "$BASE_URL/api/v1/auth/me")" == 200 ]] \
    || fail "session did not survive $1"
  login
  dc exec -T "$APP" /opt/fvoci/bin/fvoci-migrate --verify-secrets >/dev/null || fail "sealed secrets do not open after $1"
}
OLD_SESSION="$SESSION"
log_assert "== docker compose up -d again (running install)"
start_stack
key_check "a second up"
log_assert "second up: keys kept (session, password login, sealed secrets): ok"

log_assert "== docker compose down (volumes kept), then up again"
dc down
start_stack
key_check "down/up"
curl -fsS -b "$COOKIE_JAR" "$BASE_URL/api/v1/workspaces/${WORKSPACE_ID}/documents/${DOCUMENT_ID}/body" \
  | python3 -c 'import json,sys; b=json.load(sys.stdin); assert b["contentJson"]==json.loads(sys.argv[1])["contentJson"], b' "$BODY_JSON"
check_attachment
check_search
python3 "$ROOT/scripts/install-smoke-documents.py" "$BASE_URL" "$WORKSPACE_ID" "$COOKIE_JAR" "$DOCUMENT_STATE" restart
log_assert "after down/up: generated secrets kept (session, password login, sealed secrets), body, attachment, extraction, search, imports: ok"
dc down -v --remove-orphans >/dev/null

# --- settings and preparation failures keep the server down ------------------

server_down() { # reason
  if dc ps --status running --format '{{.Service}}' | grep -qx "$APP" \
    && [[ "$(docker inspect -f '{{.State.Health.Status}}' "$(dc ps -q "$APP")")" == healthy ]]; then
    fail "$APP is healthy although $1"
  fi
  if curl -fsS "$BASE_URL/ready" >/dev/null 2>&1; then fail "a server answers although $1"; fi
}

new_stack unfilled unfilled
if dc up -d >"$WORK/unfilled.log" 2>&1; then fail "up succeeded with the unfilled env.example"; fi
grep -q 'required variable .* is missing a value' "$WORK/unfilled.log" || fail "compose did not name the missing value"
[[ -z "$(dc ps -a -q)" ]] || fail "containers were created from the unfilled env.example"
log_assert "unfilled env.example as .env: compose refuses before any container exists: ok"

new_stack placeholder
set_env PASSWORD_PEPPER_KEYS '{"install":"<openssl rand -hex 32>"}'
if dc up -d --wait --wait-timeout 60 >"$WORK/placeholder.log" 2>&1; then fail "up succeeded with a placeholder value"; fi
dc logs --no-color "$APP" >"$WORK/placeholder-app.log" 2>&1
grep -q 'PASSWORD_PEPPER_KEYS still holds an example placeholder' "$WORK/placeholder-app.log" \
  || fail "the app did not name the placeholder setting"
server_down "PASSWORD_PEPPER_KEYS is a placeholder"
dc down -v --remove-orphans >/dev/null
log_assert "placeholder PASSWORD_PEPPER_KEYS: the app names it and does not start: ok"

new_stack prepare-failure
start_stack
# shellcheck disable=SC2016 # expanded by the postgres container's shell
DB_NAME="$(dc exec -T postgres sh -c 'printf %s "${POSTGRES_DB:-$POSTGRES_USER}"')"
# shellcheck disable=SC2016 # expanded by the postgres container's shell
dc exec -T postgres sh -c 'psql -X -q -U "$POSTGRES_USER" -d postgres -c "ALTER DATABASE \"$1\" SET default_transaction_read_only = on"' sh "$DB_NAME"
dc stop "$APP" >/dev/null
if dc up -d --wait --wait-timeout 60 "$APP" >"$WORK/prepare-failure.log" 2>&1; then fail "up succeeded although preparation fails"; fi
dc logs --no-color "$APP" 2>&1 | grep 'preparation failed' | tail -1 | cut -c1-200 | tee -a "$ASSERT_LOG"
server_down "preparation failed"
# shellcheck disable=SC2016 # expanded by the postgres container's shell
dc exec -T postgres sh -c 'psql -X -q -U "$POSTGRES_USER" -d postgres -c "ALTER DATABASE \"$1\" RESET default_transaction_read_only"' sh "$DB_NAME"
start_stack
dc down -v --remove-orphans >/dev/null
log_assert "failing preparation (read-only database): the server stays down, then recovers: ok"

# --- browser subset against fresh stacks --------------------------------------

if (( BROWSER )); then
  for spec in "${BROWSER_SPECS[@]}"; do
    if grep -Eq 'createE2eUser|capturedMails|process\.env|FVOCI_E2E_|execFileSync' "$ROOT/apps/web/e2e/$spec"; then
      fail "$spec needs a test fixture hook and cannot run against a release stack"
    fi
    new_stack "browser-${spec%%.*}"
    start_stack
    (cd "$ROOT/apps/web" && PLAYWRIGHT_BASE_URL="$BASE_URL" npm exec --no -- playwright test "e2e/$spec" \
      --reporter=line --output "$WORK/playwright-output")
    dc down -v --remove-orphans >/dev/null
    log_assert "browser $spec on a fresh stack: ok"
  done
else
  log_assert "browser subset skipped (--no-browser)"
fi

log_assert "== release-smoke complete for ${IMAGE_REF} on linux/${ARCH} ($((SECONDS - START_TS))s)"
cat "$ASSERT_LOG"
