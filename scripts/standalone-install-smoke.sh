#!/usr/bin/env bash
# Standalone install smoke: infra/rust/compose.user.yml alone in an empty
# folder, `docker compose up -d`, no env file. Checks secret generation,
# first-admin setup/login and search, idempotent re-up, down/up persistence,
# that the server cannot read the owner password or the Meilisearch master key,
# that a lost secret volume or a failing init keeps the server down, and a
# concurrent double `up -d` on a fresh install.
#
#   FVOCI_INSTALL_IMAGE=<built product image> scripts/standalone-install-smoke.sh
#
# The image stands in for the release reference exactly as release files do:
# the `${FVOCI_IMAGE:-...}` default is replaced, nothing else changes. The file
# publishes 127.0.0.1:8080, which must be free. Host tools: docker, curl, jq.
# Secret values are compared in memory and never printed.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${FVOCI_INSTALL_IMAGE:?FVOCI_INSTALL_IMAGE must name a built product image}"
RUN_ID="$(head -c 6 /dev/urandom | od -An -tx1 | tr -d ' \n')"
export COMPOSE_PROJECT_NAME="fvoci-standalone-smoke-${RUN_ID}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-standalone.${RUN_ID}.XXXXXX")"
JAR="$WORK/.cookies"
BASE=http://localhost:8080
ORIGIN=http://localhost:8080
SECRET_VOLUMES=(postgres_secrets meili_secrets server_secrets)
ALL_VOLUMES=("${SECRET_VOLUMES[@]}" pgdata storage searchdata)
PROJECTS=("$COMPOSE_PROJECT_NAME")

step() { printf '== %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
vol() { printf '%s_%s' "$COMPOSE_PROJECT_NAME" "$1"; }

cleanup() {
  local status=$? p v
  if (( status != 0 )); then
    (cd "$WORK" && docker compose ps -a >&2; docker compose logs --no-color --tail 80 >&2) || true
  fi
  for p in "${PROJECTS[@]}"; do
    (cd "$WORK" && COMPOSE_PROJECT_NAME="$p" docker compose down -v --remove-orphans >/dev/null 2>&1) || true
    for v in "${ALL_VOLUMES[@]}" copy; do docker volume rm -f "${p}_${v}" >/dev/null 2>&1 || true; done
  done
  rm -rf "$WORK"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

for cmd in docker curl jq; do
  command -v "$cmd" >/dev/null || fail "missing host command: $cmd"
done
docker image inspect "$IMAGE" >/dev/null || fail "image not found: $IMAGE"

# Same substitution a release file makes; everything else is the template.
sed "s|\${FVOCI_IMAGE:-ghcr.io/aisflow/fvoci:[^}]*}|${IMAGE}|" \
  "$ROOT/infra/rust/compose.user.yml" >"$WORK/compose.yml"
grep -q "image: \*fvoci-image" "$WORK/compose.yml" || fail "template lost its image anchor"
grep -q "&fvoci-image ${IMAGE}\$" "$WORK/compose.yml" || fail "image substitution failed"
[[ "$(ls -A "$WORK")" == compose.yml ]] || fail "work folder must hold only compose.yml"
cd "$WORK"

# Throwaway root reader over the three secret volumes (values never printed).
read_secrets() {
  local mounts=() v
  for v in "${SECRET_VOLUMES[@]}"; do mounts+=(-v "$(vol "$v"):/s/${v}:ro"); done
  docker run --rm --network none --user 0:0 --entrypoint sh "${mounts[@]}" "$IMAGE" -c "$1"
}
# shellcheck disable=SC2016 # expanded in the reader container
secret_values() { read_secrets 'for f in /s/*/*; do cat "$f"; echo; done'; }
# Mode, owner, sha256 and name of every file, markers included.
secret_manifest() {
  # shellcheck disable=SC2016 # expanded in the reader container
  read_secrets 'cd /s && for f in */* */.fvoci*; do
    [ -e "$f" ] || continue
    printf "%s %s %s\n" "$(stat -c "%a %u:%g" "$f")" "$(sha256sum "$f" | cut -c1-64)" "$f"; done'
}
service_state() {
  local cid
  cid="$(docker compose ps -a -q "$1")"
  [[ -n "$cid" ]] || { echo "absent"; return; }
  docker inspect -f '{{.State.Status}} {{.State.ExitCode}} {{.State.StartedAt}}' "$cid"
}
never_started() { [[ "$(service_state "$1")" == absent || "$(service_state "$1")" == *" 0001-01-01T00:00:00Z" ]]; }
wait_healthy() {
  local deadline=$((SECONDS + 180)) cid
  while (( SECONDS < deadline )); do
    cid="$(docker compose ps -q fvoci)"
    if [[ -n "$cid" && "$(docker inspect -f '{{.State.Health.Status}}' "$cid")" == healthy ]]; then
      return 0
    fi
    sleep 2
  done
  fail "fvoci did not become healthy"
}
login() {
  curl -fsS -c "$JAR" -b "$JAR" -H 'content-type: application/json' -H "origin: $ORIGIN" \
    -X POST "$BASE/api/v1/auth/login" -d '{"email":"owner@standalone.test","password":"standalonepass1"}' \
    | jq -e '.userId' >/dev/null
}
# up -d that is expected to fail; prints the last lines.
up_fails() {
  local out status
  set +e
  out="$(docker compose up -d 2>&1)"
  status=$?
  set -e
  printf '%s\n' "$out" | tail -2
  (( status != 0 )) || fail "up -d succeeded: $1"
}
copy_volume() { # from to
  docker run --rm --network none --user 0:0 -v "$1:/from:ro" -v "$2:/to" --entrypoint sh "$IMAGE" \
    -c 'rm -rf /to/..?* /to/.[!.]* /to/*; cp -a /from/. /to/'
}

step "first up -d (compose.yml only, no env file)"
T0=$SECONDS
docker compose up -d
wait_healthy
echo "healthy after $((SECONDS - T0))s"
docker compose config --services | sort | tr '\n' ' '; echo
[[ "$(docker compose config --services | sort | tr '\n' ' ')" == "fvoci init meilisearch postgres " ]] \
  || fail "unexpected services"
[[ "$(service_state init)" == "exited 0 "* ]] || fail "init: $(service_state init)"
docker compose logs --no-color init | grep -q "generated server keys" || fail "init did not generate the server keys"
docker compose logs --no-color postgres | grep -q "fvoci: generated /run/fvoci/secrets/postgres_password" || fail "postgres did not generate"
docker compose logs --no-color meilisearch | grep -q "fvoci: generated /run/fvoci/secrets/master_key" || fail "meilisearch did not generate"
echo "postgres, meilisearch and init generated their secrets; init exited 0"

step "published ports"
PUBLISHED="$(docker compose ps --format json | jq -rs '[.[] | .Publishers[]? | select(.PublishedPort > 0) | "\(.URL):\(.PublishedPort)->\(.TargetPort)"] | unique | join(" ")')"
[[ "$PUBLISHED" == "127.0.0.1:8080->8080" ]] || fail "unexpected published ports: $PUBLISHED"
echo "$PUBLISHED"

step "secret files: mode and owner"
MANIFEST1="$(secret_manifest)"
awk '{print $1, $2, $4}' <<<"$MANIFEST1"
# postgres_password and master_key: their service's uid, group 1001 (init),
# 0640; the server's files and marker: uid/gid 1000, 0600.
awk '
  { want = "unexpected file" }
  $4 == "postgres_secrets/postgres_password" { want = "640 999:1001" }
  $4 == "meili_secrets/master_key"           { want = "640 0:1001" }
  $4 ~ /^server_secrets\//                   { want = "600 1000:1000" }
  { if ($1 " " $2 != want) bad = bad " " $4 }
  END { if (bad != "") { print "wrong mode/owner:" bad > "/dev/stderr"; exit 1 } }
' <<<"$MANIFEST1" || fail "secret file mode or owner"
for f in database_app_url password_pepper_keys password_pepper_active_key_id encryption_keys \
  encryption_active_key_id meili_url meili_api_key .fvoci-install-complete; do
  grep -q " server_secrets/$f\$" <<<"$MANIFEST1" || fail "server_secrets/$f missing"
done
[[ "$(wc -l <<<"$MANIFEST1")" == 10 ]] || fail "expected 10 secret files"
mapfile -t SECRETS < <(secret_values | awk 'length($0) >= 32')
(( ${#SECRETS[@]} >= 6 )) || fail "expected at least 6 long secret values, got ${#SECRETS[@]}"
OWNER_PW="$(read_secrets 'cat /s/postgres_secrets/postgres_password')"
MASTER_KEY="$(read_secrets 'cat /s/meili_secrets/master_key')"

step "the server cannot read the owner password or the Meilisearch master key"
SERVER_CID="$(docker compose ps -q fvoci)"
docker compose exec -T fvoci id
[[ "$(docker compose exec -T fvoci id -G)" == 1000 ]] || fail "server has supplementary groups"
docker compose exec -T fvoci ls -A /run/fvoci/secrets
for path in /run/fvoci/secrets/postgres_password /run/fvoci/secrets/master_key \
  /run/fvoci/install/postgres/postgres_password /run/fvoci/install/meilisearch/master_key; do
  if docker compose exec -T fvoci cat "$path" >/dev/null 2>&1; then fail "server can read $path"; fi
  echo "fvoci: cat $path -> refused (absent)"
done
MOUNTS="$(docker inspect -f '{{range .Mounts}}{{.Name}}:{{.Destination}}:{{.RW}} {{end}}' "$SERVER_CID")"
echo "server mounts: $MOUNTS"
[[ "$MOUNTS" != *postgres_secrets* && "$MOUNTS" != *meili_secrets* ]] || fail "server mounts an owner secret volume"
# environ, argv, every readable file under /run/fvoci and the targets of every
# open file descriptor of the server process and its children.
SERVER_VIEW="$(docker compose exec -T fvoci sh -c '
  for p in /proc/[0-9]*; do tr "\0" "\n" <"$p/environ"; tr "\0" " " <"$p/cmdline"; echo; ls -l "$p/fd" 2>/dev/null; done
  find /run/fvoci -type f -exec cat {} + 2>/dev/null; echo' 2>/dev/null)"
grep -qF "$OWNER_PW" <<<"$SERVER_VIEW" && fail "owner password visible to the server"
grep -qF "$MASTER_KEY" <<<"$SERVER_VIEW" && fail "master key visible to the server"
grep -Eq 'postgres_password|master_key' <<<"$SERVER_VIEW" && fail "server holds a descriptor or path of an owner secret"
docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$SERVER_CID" \
  | grep -E '^(POSTGRES_PASSWORD|MEILI_MASTER_KEY|DATABASE_URL|DATABASE_APP_URL|PASSWORD_PEPPER_KEYS|ENCRYPTION_KEYS)=' \
  && fail "secret value in server container config"
echo "owner password and master key absent from server environ/argv/fds/files/config: ok"
docker compose exec -T fvoci /opt/fvoci/bin/fvoci-migrate --doctor | jq -e '.ok == true' >/dev/null \
  || fail "doctor in the server container"
echo "doctor in the server container (install settings files): ok"

step "first-admin setup, login and search over HTTP"
curl -fsS "$BASE/api/v1/setup" | jq -c .
curl -fsS -c "$JAR" -b "$JAR" -H 'content-type: application/json' -H "origin: $ORIGIN" \
  -X POST "$BASE/api/v1/setup" \
  -d '{"email":"owner@standalone.test","password":"standalonepass1","givenName":"Owner","workspaceSlug":"standalone","workspaceName":"Standalone"}' >/dev/null
login
WS="$(curl -fsS -b "$JAR" "$BASE/api/v1/me/workspaces" | jq -er '.items[0].id')"
DOC="$(curl -fsS -b "$JAR" -H 'content-type: application/json' -H "origin: $ORIGIN" \
  -X POST "$BASE/api/v1/workspaces/${WS}/documents" -d '{"parentId":null,"title":"Standalone doc"}' | jq -er .id)"
curl -fsS "$BASE/" | grep -qi '<!doctype html' || fail "React build not served at /"
deadline=$((SECONDS + 60))
until curl -fsS -b "$JAR" "$BASE/api/v1/workspaces/${WS}/search?q=Standalone%20doc" \
  | jq -e --arg d "$DOC" '.items | map(.documentId // .id) | index($d) != null' >/dev/null; do
  (( SECONDS < deadline )) || fail "search did not find the document"
  sleep 1
done
echo "setup + login + document create + search + web root: ok"

step "second up -d leaves secrets byte-identical"
docker compose up -d
wait_healthy
docker compose logs --no-color init | grep -q "server keys present in /run/fvoci/install/server; kept" \
  || fail "init did not keep the server keys"
[[ "$(secret_manifest)" == "$MANIFEST1" ]] || fail "secret files changed on second up"
echo "secret manifest identical after second up: ok"

step "down + up -d keeps data and keys"
docker compose down
docker compose up -d
wait_healthy
[[ "$(secret_manifest)" == "$MANIFEST1" ]] || fail "secret files changed after down/up"
rm -f "$JAR"
login
curl -fsS -b "$JAR" "$BASE/api/v1/workspaces/${WS}/documents/${DOC}" | jq -e '.title == "Standalone doc"' >/dev/null \
  || fail "document lost"
curl -fsS "$BASE/api/v1/setup" | jq -e '.needed == false' >/dev/null || fail "setup needed again"
echo "login with the same pepper, workspace and document survive down/up: ok"

step "no secret value in any container log"
LOGS="$(docker compose logs --no-color 2>&1)"
for s in "${SECRETS[@]}"; do
  grep -qF "$s" <<<"$LOGS" && fail "a secret value appears in container logs"
done
echo "checked ${#SECRETS[@]} secret values against $(wc -l <<<"$LOGS") log lines: none found"

step "init failure (grants refused by a read-only database) keeps the server down"
docker compose exec -T postgres psql -X -q -U fvoci_owner -d postgres \
  -c 'ALTER DATABASE fvoci SET default_transaction_read_only = on'
docker compose down
up_fails "init failed"
[[ "$(service_state init)" == "exited 1 "* ]] || fail "init: $(service_state init)"
docker compose logs --no-color init | grep -E 'read-only transaction' | tail -1 | cut -c1-200
never_started fvoci || fail "fvoci started although init failed: $(service_state fvoci)"
docker compose exec -T postgres psql -X -q -U fvoci_owner -d postgres \
  -c 'ALTER DATABASE fvoci RESET default_transaction_read_only'
docker compose up -d
wait_healthy
echo "failed init: up -d nonzero, fvoci never started; after the fix up -d recovers: ok"

# Losing one secret volume while its data exists: the owning service refuses,
# nothing downstream starts and nothing is generated. Then the saved copy is
# put back (a volume restore) and the install starts with the same keys.
for v in server_secrets postgres_secrets meili_secrets; do
  step "$v removed while data exists: refused, server does not start; restored copy starts"
  docker compose down
  docker volume create "$(vol copy)" >/dev/null
  copy_volume "$(vol "$v")" "$(vol copy)"
  docker volume rm "$(vol "$v")" >/dev/null
  up_fails "$v missing"
  case "$v" in
    server_secrets)
      [[ "$(service_state init)" == "exited 1 "* ]] || fail "init: $(service_state init)"
      docker compose logs --no-color init | grep -o 'refusing to generate new server keys: [^(]*(role [a-z_]*)' | tail -1
      ;;
    postgres_secrets)
      docker compose logs --no-color postgres | grep -q 'postgres_password is missing but /var/lib/postgresql holds data' \
        || fail "postgres did not refuse"
      never_started init || fail "init started without the owner password"
      echo "postgres refused: postgres_password missing while /var/lib/postgresql holds data"
      ;;
    meili_secrets)
      docker compose logs --no-color meilisearch | grep -q 'master_key is missing but /meili_data holds data' \
        || fail "meilisearch did not refuse"
      never_started init || fail "init started without the master key"
      echo "meilisearch refused: master_key missing while /meili_data holds data"
      ;;
  esac
  never_started fvoci || fail "fvoci started without $v: $(service_state fvoci)"
  n="$(docker run --rm --network none -v "$(vol "$v"):/s:ro" --entrypoint sh "$IMAGE" -c 'ls -A /s | wc -l')"
  [[ "$n" == 0 ]] || fail "a new secret was written into $v despite existing data"
  docker compose down
  copy_volume "$(vol copy)" "$(vol "$v")"
  docker volume rm "$(vol copy)" >/dev/null
  docker compose up -d
  wait_healthy
  [[ "$(secret_manifest)" == "$MANIFEST1" ]] || fail "secrets differ after restoring $v"
  rm -f "$JAR"
  login
  echo "$v: refused while missing (nothing generated); restored volume starts with identical keys and login: ok"
done

step "concurrent double up -d on a fresh install"
docker compose down -v
FRESH="${COMPOSE_PROJECT_NAME}-race"
PROJECTS+=("$FRESH")
set +e
COMPOSE_PROJECT_NAME="$FRESH" docker compose up -d >"$WORK/race1.log" 2>&1 &
P1=$!
COMPOSE_PROJECT_NAME="$FRESH" docker compose up -d >"$WORK/race2.log" 2>&1 &
P2=$!
wait "$P1"; S1=$?
wait "$P2"; S2=$?
set -e
echo "concurrent up -d exit codes: $S1 $S2"
tail -2 "$WORK/race1.log" "$WORK/race2.log"
export COMPOSE_PROJECT_NAME="$FRESH"
docker compose up -d
wait_healthy
RACE_MANIFEST="$(secret_manifest)"
[[ "$(wc -l <<<"$RACE_MANIFEST")" == 10 ]] || fail "race left an incomplete secret set"
[[ "$(docker compose logs --no-color init | grep -c 'generated server keys')" -le 1 ]] || fail "server keys generated twice"
curl -fsS -c "$JAR" -b "$JAR" -H 'content-type: application/json' -H "origin: $ORIGIN" \
  -X POST "$BASE/api/v1/setup" \
  -d '{"email":"owner@standalone.test","password":"standalonepass1","givenName":"Owner","workspaceSlug":"race","workspaceName":"Race"}' >/dev/null
rm -f "$JAR"
login
docker compose up -d
wait_healthy
[[ "$(secret_manifest)" == "$RACE_MANIFEST" ]] || fail "secrets changed after the race install"
echo "double up -d: one key set, setup + login, stable on re-up: ok"

step "standalone install smoke passed"
