#!/usr/bin/env bash
# Install smoke for the user procedure of infra/rust/compose.user.yml: an empty
# folder with compose.yml and env.example, `cp env.example .env`, fill in every
# empty value as its comments show, `docker compose up -d`. Checks that an
# unfilled, placeholder or malformed .env is refused, which .env values each
# service's environment gets, the startup preparation and the server's
# restricted process, first-admin setup/login/search, restart, recreate and
# down/up persistence, refused changed passwords, preparation failures and
# signals keeping the server down, upgrade refusal while a server is live,
# backup/restore, and concurrent starts.
#
#   FVOCI_INSTALL_IMAGE=<built product image> scripts/standalone-install-smoke.sh
#
# The image stands in for the release reference exactly as release files do:
# the `${FVOCI_IMAGE:-...}` default is replaced, nothing else changes. The file
# publishes 127.0.0.1:8080, which must be free. Host tools: docker, curl, jq,
# bun. Secret values are compared in memory and never printed.
# Each `step` is a phase (tools/install-smoke/smoke.ts). The trap removes every
# project the run created on success, failure and INT/TERM and fails a passing
# run if a container, volume or network of one remains. Two runs on one host
# collide on 127.0.0.1:8080: that port is part of the user compose file under test.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Trap glue only: phases, the first error and log groups are
# tools/install-smoke/smoke.ts; image identity, running-image and leftover
# checks are `cargo xtask install-image`.
smoke_ts() { bun "$ROOT/tools/install-smoke/smoke.ts" "$@"; }
xtask() { cargo run --quiet --locked --manifest-path "$ROOT/xtask/Cargo.toml" -- "$@"; }
json_field() { smoke_ts field "$@"; }
smoke_check() { smoke_ts check "$@"; }
SMOKE_STATE="$(mktemp "${TMPDIR:-/tmp}/standalone-install-smoke-state.XXXXXX")"
smoke_ts init "$SMOKE_STATE" standalone-install-smoke
phase() { smoke_ts phase "$SMOKE_STATE" "$1"; }
fail() { smoke_ts fail "$SMOKE_STATE" "$*"; exit 1; }
set -E
trap 'SMOKE_ERR=$?; [[ $BASHPID != "$$" ]] || smoke_ts error "$SMOKE_STATE" "$SMOKE_ERR" "$LINENO" "$BASH_COMMAND"' ERR
IMAGE="${FVOCI_INSTALL_IMAGE:?FVOCI_INSTALL_IMAGE must name a built product image}"
RUN_ID="$(head -c 6 /dev/urandom | od -An -tx1 | tr -d ' \n')"
export COMPOSE_PROJECT_NAME="fvoci-install-smoke-${RUN_ID}"
MAIN="$COMPOSE_PROJECT_NAME"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-install.${RUN_ID}.XXXXXX")"
JAR="$WORK/.cookies"
BASE=http://localhost:8080
ORIGIN=http://localhost:8080
PROJECTS=("$MAIN")

step() { phase "$*"; }

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
  local status=$? p torn=0
  set +e
  trap - ERR
  smoke_ts report "$SMOKE_STATE" "$status"
  if [[ -f "$WORK/compose.yml" ]]; then
    if (( status != 0 )); then
      smoke_ts group "collect: compose ps and logs (last 80 lines)"
      (cd "$WORK" && docker compose ps -a; docker compose logs --no-color --tail 80) 2>&1 | smoke_ts quote >&2
      smoke_ts endgroup
    fi
    smoke_ts group "cleanup: ${PROJECTS[*]}"
    for p in "${PROJECTS[@]}"; do
      teardown "$p" docker compose --project-directory "$WORK" -f "$WORK/compose.yml" -p "$p" || torn=1
    done
    smoke_ts endgroup
  fi
  (( torn == 0 || status != 0 )) || status=1
  rm -rf "$WORK"
  smoke_ts finish "$SMOKE_STATE" "$status"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

for cmd in docker curl jq bun; do
  command -v "$cmd" >/dev/null || fail "missing host command: $cmd"
done
docker image inspect "$IMAGE" >/dev/null || fail "image not found: $IMAGE"

# The release files: compose.yml with the image substituted, env.example.
sed "s|\${FVOCI_IMAGE:-ghcr.io/aisflow/fvoci:[^}]*}|${IMAGE}|" \
  "$ROOT/infra/rust/compose.user.yml" >"$WORK/compose.yml"
grep -q "&fvoci-image ${IMAGE}\$" "$WORK/compose.yml" || fail "image substitution failed"
cp "$ROOT/infra/rust/compose.user.env.example" "$WORK/env.example"
cd "$WORK"

# .env from env.example as the user procedure fills it, and single-key edits
# (tools/install-smoke/smoke.ts fill-env / set-env).
fill_env() { smoke_ts fill-env env.example .env || fail "cannot fill .env from env.example"; }
env_value() { sed -n "s/^$1=//p" .env; }
set_env() { smoke_ts set-env .env "$1" "$2" || fail "cannot set $1 in .env"; }
logs() { docker compose logs --no-color "$1" 2>&1; }
state() {
  local cid
  cid="$(docker compose ps -a -q "$1")"
  [[ -n "$cid" ]] || { echo absent; return; }
  docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' "$cid"
}
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
# The app fails and does not serve; waits for the given log line first.
wait_refused() { # log-pattern
  local deadline=$((SECONDS + 90))
  until grep -q "$1" <<<"$(logs fvoci)"; do
    (( SECONDS < deadline )) || fail "fvoci never logged: $1"
    sleep 1
  done
  grep "$1" <<<"$(logs fvoci)" | tail -1 | cut -c1-220
  if curl -fsS "$BASE/ready" >/dev/null 2>&1; then fail "a server answers although: $1"; fi
  [[ "$(docker inspect -f '{{.State.Health.Status}}' "$(docker compose ps -a -q fvoci)")" != healthy ]] \
    || fail "fvoci is healthy although: $1"
}
login() {
  curl -fsS -c "$JAR" -b "$JAR" -H 'content-type: application/json' -H "origin: $ORIGIN" \
    -X POST "$BASE/api/v1/auth/login" -d '{"email":"owner@install.test","password":"installpass1"}' \
    | jq -e '.userId' >/dev/null
}
psql_owner() { # SQL (on the maintenance db, so a read-only fvoci db can be changed)
  docker compose exec -T postgres psql -X -q -tA -U fvoci_owner -d "${2:-postgres}" -c "$1"
}

step "no .env, then the unfilled env.example as .env: compose refuses, nothing is created"
[[ "$(find . -mindepth 1 -printf '%f\n' | sort | tr '\n' ' ')" == "compose.yml env.example " ]] \
  || fail "work folder must hold compose.yml and env.example"
for attempt in none unfilled; do
  [[ "$attempt" == none ]] || cp env.example .env
  set +e
  OUT="$(docker compose up -d 2>&1)"
  STATUS=$?
  set -e
  (( STATUS != 0 )) || fail "up -d succeeded with $attempt .env"
  grep -o 'required variable [A-Z_]* is missing a value: [^"]*' <<<"$OUT" | head -1
  [[ -z "$(docker compose ps -a -q)" && -z "$(docker volume ls -q --filter "name=${MAIN}_")" ]] \
    || fail "containers or volumes created with $attempt .env"
done
echo "compose refused both before creating anything: ok"

step "a placeholder value, then a malformed key: the app names it and the server does not start"
fill_env
PEPPER="$(env_value PASSWORD_PEPPER_KEYS)"
set_env PASSWORD_PEPPER_KEYS '{"install":"<openssl rand -hex 32>"}'
docker compose up -d
wait_refused 'PASSWORD_PEPPER_KEYS still holds an example placeholder'
set_env PASSWORD_PEPPER_KEYS "$PEPPER"
ENC="$(env_value ENCRYPTION_KEYS)"
set_env ENCRYPTION_KEYS '{"install":"00"}'
# A changed .env value reaches the container only when Compose recreates it.
docker compose up -d
wait_refused 'ENCRYPTION_KEYS / ENCRYPTION_ACTIVE_KEY_ID:'
set_env ENCRYPTION_KEYS "$ENC"
docker compose down -v
echo "placeholder and malformed keyring refused by the app (nothing prepared): ok"

step "docker compose up -d with the filled .env"
T0=$SECONDS
docker compose up -d
wait_healthy
echo "healthy after $((SECONDS - T0))s"
[[ "$(docker compose config --services | sort | tr '\n' ' ')" == "fvoci meilisearch postgres " ]] \
  || fail "unexpected services"
PREP_LOG="$(logs fvoci | grep 'fvoci:')"
printf '%s\n' "$PREP_LOG" | cut -c1-160
for line in 'ready; preparing' 'created app role fvoci_app' 'migrated; granted app role privileges' \
  'search key ready' 'prepared; starting the server'; do
  grep -q "$line" <<<"$PREP_LOG" || fail "startup did not log: $line"
done
PUBLISHED="$(docker compose ps --format json | jq -rs '[.[] | .Publishers[]? | select(.PublishedPort > 0) | "\(.URL):\(.PublishedPort)->\(.TargetPort)"] | unique | join(" ")')"
[[ "$PUBLISHED" == "127.0.0.1:8080->8080" ]] || fail "unexpected published ports: $PUBLISHED"
echo "published: $PUBLISHED"

step "each service's environment: only the .env values it needs (names printed, values compared in memory)"
OWNER_PW="$(env_value POSTGRES_PASSWORD)"
MASTER_KEY="$(env_value MEILI_MASTER_KEY)"
APP_PW="$(env_value FVOCI_APP_PASSWORD)"
PEPPER_KEYS="$(env_value PASSWORD_PEPPER_KEYS)"
ENC_KEYS="$(env_value ENCRYPTION_KEYS)"
has_secret() { grep -qF -e "$OWNER_PW" -e "$MASTER_KEY" -e "$APP_PW"; }
has_keyring() { grep -qF -e "$PEPPER_KEYS" -e "$ENC_KEYS"; }
# docker inspect escapes the keyrings' quotes: match their key material.
key_hex() { local v="${1#*\":\"}"; printf '%s' "${v%\"\}}"; }
PEPPER_HEX="$(key_hex "$PEPPER_KEYS")"
ENC_HEX="$(key_hex "$ENC_KEYS")"
(( ${#PEPPER_HEX} == 64 && ${#ENC_HEX} == 64 )) || fail "could not take the key material from the keyrings"
ENV_KEYS="$(sed -n 's/^\([A-Z][A-Z0-9_]*\)=.*/\1/p' .env | LC_ALL=C sort)"
config_env() { docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$(docker compose ps -q "$1")"; }
env_names() { # service: the .env names in its container configuration
  config_env "$1" | sed -n 's/=.*//p' | LC_ALL=C sort | LC_ALL=C comm -12 - <(printf '%s\n' "$ENV_KEYS") | tr '\n' ' '
}
for svc in fvoci postgres meilisearch; do
  printf '  %s: %s\n' "$svc" "$(env_names "$svc")"
  if config_env "$svc" | sed -n 's/=.*//p' | grep -q '_FILE$'; then fail "$svc is configured with a *_FILE setting"; fi
done
[[ "$(env_names fvoci)" == "ENCRYPTION_ACTIVE_KEY_ID ENCRYPTION_KEYS FVOCI_APP_PASSWORD FVOCI_PUBLIC_ORIGIN MEILI_MASTER_KEY PASSWORD_PEPPER_ACTIVE_KEY_ID PASSWORD_PEPPER_KEYS POSTGRES_PASSWORD " ]] \
  || fail "fvoci does not get exactly its .env values"
[[ "$(env_names postgres)" == "POSTGRES_PASSWORD " ]] || fail "postgres gets .env values other than POSTGRES_PASSWORD"
[[ "$(env_names meilisearch)" == "MEILI_MASTER_KEY " ]] || fail "meilisearch gets .env values other than MEILI_MASTER_KEY"
# No value reaches another service under another name, or its command line.
grep -qF -e "$APP_PW" -e "$MASTER_KEY" -e "$PEPPER_HEX" -e "$ENC_HEX" <<<"$(docker inspect "$(docker compose ps -q postgres)")" \
  && fail "postgres's configuration holds a value it does not need"
grep -qF -e "$OWNER_PW" -e "$APP_PW" -e "$PEPPER_HEX" -e "$ENC_HEX" <<<"$(docker inspect "$(docker compose ps -q meilisearch)")" \
  && fail "meilisearch's configuration holds a value it does not need"
echo "fvoci: its eight .env values; postgres: the owner password; meilisearch: the master key; no *_FILE setting: ok"

step "uid boundary: the server (uid 1000) cannot reach the owner password, master key or raw app password"
CID="$(docker compose ps -q fvoci)"
[[ "$(docker exec "$CID" id -u)" == 0 ]] || fail "docker exec in fvoci does not default to root"
# The server is non-dumpable: the kernel owns its /proc entries (exe, environ,
# fd) by root and requires CAP_SYS_PTRACE, so neither uid 1000 (its helpers, a
# uid-1000 exec) nor root in the container (Docker's default capabilities) can
# read them. Only a privileged exec can; argv stays world-readable.
[[ "$(docker exec --user 1000:1000 "$CID" sh -c 'tr "\0" "\n" </proc/1/cmdline | head -n 1')" == /opt/fvoci/bin/fvoci-server ]] || fail "pid 1 is not fvoci-server"
[[ "$(docker exec "$CID" stat -c '%u' /proc/1/environ)" == 0 ]] || fail "the server's /proc files are not root-owned: the server is dumpable"
if docker exec "$CID" cat /proc/1/environ >/dev/null 2>&1; then fail "expected root in the container to be unable to read the server environ"; fi
if docker exec --user 1000:1000 "$CID" cat /proc/1/environ >/dev/null 2>&1; then fail "uid 1000 can read the server environ"; fi
if docker exec --user 1000:1000 "$CID" ls /proc/1/fd >/dev/null 2>&1; then fail "uid 1000 can list the server's descriptors"; fi
echo "server is non-dumpable: neither uid 1000 nor unprivileged root reads its environ or descriptors: ok"
PID1="$(docker exec "$CID" sh -c 'grep -E "^(Uid|Gid|Groups|CapPrm|CapEff|CapAmb|NoNewPrivs):" /proc/1/status')"
printf '%s\n' "$PID1"
grep -Eq '^Uid:[[:space:]]+1000[[:space:]]+1000[[:space:]]+1000[[:space:]]+1000$' <<<"$PID1" || fail "server uids are not all 1000"
grep -Eq '^Gid:[[:space:]]+1000[[:space:]]+1000[[:space:]]+1000[[:space:]]+1000$' <<<"$PID1" || fail "server gids are not all 1000"
grep -Eq '^Groups:[[:space:]]*$' <<<"$PID1" || fail "server keeps supplementary groups"
grep -Eq '^CapEff:[[:space:]]+0+$' <<<"$PID1" || fail "server keeps effective capabilities"
grep -Eq '^CapPrm:[[:space:]]+0+$' <<<"$PID1" || fail "server keeps permitted capabilities"
grep -Eq '^NoNewPrivs:[[:space:]]+1$' <<<"$PID1" || fail "server runs without no-new-privileges"
[[ -z "$(docker exec "$CID" find / -xdev -perm /6000 -type f)" ]] || fail "the image has setuid/setgid files"
echo "no-new-privileges and no setuid/setgid file: ok"
# The server's own tree: environ, argv and fds of pid 1 and its descendants,
# read by a privileged exec (root with CAP_SYS_PTRACE, since the server is
# non-dumpable); files under /run, read as the server's uid 1000.
# shellcheck disable=SC2016 # expanded in the app container
PROC_VIEW="$(docker exec --privileged --user 0:0 "$CID" sh -c '
  for p in /proc/[0-9]*; do
    a=${p#/proc/}
    while [ "$a" != 1 ] && [ "$a" != 0 ] && [ -n "$a" ]; do a=$(sed -n "s/^PPid:[[:space:]]*//p" "/proc/$a/status" 2>/dev/null); done
    [ "$a" = 1 ] || continue
    echo "pid ${p#/proc/}: $(tr "\0" " " <"$p/cmdline")"
    tr "\0" "\n" <"$p/environ"; ls -l "$p/fd"
  done 2>/dev/null; true')"
RUN_VIEW="$(docker exec --user 1000:1000 "$CID" sh -c 'find /run -type f -exec cat {} + 2>/dev/null; echo')"
VIEW="${PROC_VIEW}"$'\n'"${RUN_VIEW}"
grep '^pid ' <<<"$VIEW"
# The view must hold the server's environ, or the negative checks below pass
# on an empty read.
grep -q '^DATABASE_APP_URL=postgres://fvoci_app:' <<<"$VIEW" || fail "the privileged view lacks the server's app role URL"
grep -qF -e "$OWNER_PW" -e "$MASTER_KEY" <<<"$VIEW" && fail "owner password or master key in the server process tree"
grep -Eq '^(POSTGRES_PASSWORD|FVOCI_APP_PASSWORD|MEILI_MASTER_KEY|FVOCI_MEILI_MASTER_KEY|DATABASE_URL|FVOCI_MIGRATION_URL)=' <<<"$VIEW" \
  && fail "a preparation-only variable reached the server"
if ! grep -qxF "PASSWORD_PEPPER_KEYS=$PEPPER_KEYS" <<<"$VIEW" || ! grep -qxF "ENCRYPTION_KEYS=$ENC_KEYS" <<<"$VIEW"; then
  fail "the server lacks its keyrings"
fi
echo "server tree: keyrings and DATABASE_APP_URL; no owner password, master key or other preparation-only variable in environ/argv/fds or /run files: ok"
# Healthcheck and docker exec processes start from the container
# configuration, so they hold every value Compose passes to fvoci, the
# preparation's included, and run as root. Hold one open (as the healthcheck
# does) and confirm that; then everything uid 1000 can read under /proc must
# hold none of it.
docker exec -d "$CID" sh -c 'exec sleep 30'
sleep 1
EXEC_ENV="$(docker exec "$CID" sh -c 'for p in /proc/[0-9]*; do [ "$(cat $p/comm 2>/dev/null)" = sleep ] && tr "\0" "\n" <"$p/environ"; done; true')"
[[ -n "$EXEC_ENV" ]] || fail "no docker exec process found"
has_secret <<<"$EXEC_ENV" || fail "the root docker exec process lacks the configured passwords, so the uid 1000 check below proves nothing"
# The probe itself is a docker exec and so would start with every
# configured value: it clears its environment first.
# shellcheck disable=SC2016 # expanded in the app container
UID_VIEW="$(docker exec --user 1000:1000 "$CID" env -i PATH=/usr/bin:/bin sh -c 'for p in /proc/[0-9]*; do tr "\0" "\n" <"$p/environ"; tr "\0" " " <"$p/cmdline"; echo; done 2>/dev/null')"
grep -q '^PATH=' <<<"$UID_VIEW" || fail "uid 1000 read no environ at all under /proc"
# The server's DATABASE_APP_URL (with the app password) and keyrings are out of
# reach too: the server is non-dumpable and its helpers start without its env.
has_secret <<<"$UID_VIEW" && fail "uid 1000 reads the owner password, master key or app password under /proc"
has_keyring <<<"$UID_VIEW" && fail "uid 1000 reads a keyring under /proc"
echo "root docker exec / healthcheck processes hold the configured values; uid 1000 finds no secret or keyring under /proc: ok"
docker compose exec -T fvoci /opt/fvoci/bin/fvoci-migrate --doctor | jq -e '.ok == true' >/dev/null || fail "doctor"
echo "doctor in the fvoci container: ok"

step "search key directory: uid 1000 cannot redirect root's key write"
KEYDIR=/run/fvoci/meili
key_layout() { docker exec "$CID" stat -c '%u %g %a %F' "$KEYDIR" "$KEYDIR/api_key" | tr '\n' ';'; }
[[ "$(key_layout)" == "0 0 755 directory;0 1000 640 regular file;" ]] \
  || fail "key directory/file are not root:root 0755 / root:1000 0640: $(key_layout)"
docker exec --user 1000:1000 "$CID" test -r "$KEYDIR/api_key" || fail "uid 1000 cannot read the search key"
# A root-only file (root:shadow 0640) stands in for anything root may write
# and uid 1000 may not read.
TARGET=/etc/shadow
for attempt in "ln -s $TARGET $KEYDIR/x" ": >$KEYDIR/x" "rm -f $KEYDIR/api_key" \
  "chmod 666 $KEYDIR/api_key" ": >$KEYDIR/api_key"; do
  if docker exec --user 1000:1000 "$CID" sh -c "$attempt" 2>/dev/null; then fail "uid 1000 could: $attempt"; fi
done
# An earlier release's volume (or a server that ran before root took the
# directory over) is uid 1000's: plant symlinks where root writes, then run
# root's key write as an operator would (docker compose exec defaults to root).
target_state() { docker exec "$CID" sh -c "stat -c '%u %g %a %s' $TARGET; sha256sum <$TARGET"; }
TARGET_BEFORE="$(target_state)"
if docker exec --user 1000:1000 "$CID" cat "$TARGET" >/dev/null 2>&1; then fail "uid 1000 can read $TARGET"; fi
KEY_BEFORE="$(docker exec "$CID" cat "$KEYDIR/api_key")"
docker exec "$CID" chown 1000:1000 "$KEYDIR"
docker exec --user 1000:1000 "$CID" sh -c "rm -f $KEYDIR/api_key \
  && ln -s $TARGET $KEYDIR/api_key \
  && ln -s $TARGET $KEYDIR/api_key.tmp \
  && ln -s $TARGET $KEYDIR/.api_key.tmp"
docker exec "$CID" ls -ln "$KEYDIR" | sed 's/^/  planted: /'
docker exec "$CID" /opt/fvoci/bin/fvoci-migrate --ensure-meili-key "$KEYDIR/api_key" || fail "root --ensure-meili-key over planted links"
[[ "$(target_state)" == "$TARGET_BEFORE" ]] || fail "the planted link changed $TARGET"
if docker exec --user 1000:1000 "$CID" cat "$TARGET" >/dev/null 2>&1; then
  fail "uid 1000 can read $TARGET after the planted link"
fi
[[ "$(key_layout)" == "0 0 755 directory;0 1000 640 regular file;" ]] \
  || fail "root did not take the directory back or replace the link: $(key_layout)"
[[ "$(docker exec "$CID" cat "$KEYDIR/api_key")" == "$KEY_BEFORE" ]] || fail "the scoped key changed"
docker exec "$CID" find "$KEYDIR" -type l -delete
echo "planted symlinks replaced, not followed; $TARGET unchanged; directory root-owned again: ok"

step "first-admin setup, login and search"
curl -fsS "$BASE/api/v1/setup" | jq -c .
curl -fsS -c "$JAR" -b "$JAR" -H 'content-type: application/json' -H "origin: $ORIGIN" -X POST "$BASE/api/v1/setup" \
  -d '{"email":"owner@install.test","password":"installpass1","givenName":"Owner","workspaceSlug":"install","workspaceName":"Install"}' >/dev/null
login
WS="$(curl -fsS -b "$JAR" "$BASE/api/v1/me/workspaces" | jq -er '.items[0].id')"
DOC_COMMAND_ID="$(smoke_ts uuid)"
DOC_CREATE_BODY="{\"commandId\":\"${DOC_COMMAND_ID}\",\"parentId\":null,\"title\":\"Install doc\"}"
DOC="$(curl -fsS -b "$JAR" -H 'content-type: application/json' -H "origin: $ORIGIN" \
  -X POST "$BASE/api/v1/workspaces/${WS}/documents" -d "$DOC_CREATE_BODY" | jq -er .id)"
curl -fsS "$BASE/" | grep -qi '<!doctype html' || fail "web root"
deadline=$((SECONDS + 60))
until curl -fsS -b "$JAR" "$BASE/api/v1/workspaces/${WS}/search?q=Install%20doc" \
  | jq -e --arg d "$DOC" '.items | map(.documentId // .id) | index($d) != null' >/dev/null; do
  (( SECONDS < deadline )) || fail "search did not find the document"
  sleep 1
done
echo "setup, login, document, search, web root: ok"

check_same_install() { # after
  rm -f "$JAR"
  login
  curl -fsS -b "$JAR" "$BASE/api/v1/workspaces/${WS}/documents/${DOC}" | jq -e '.title == "Install doc"' >/dev/null \
    || fail "document lost after $1"
  curl -fsS "$BASE/api/v1/setup" | jq -e '.needed == false' >/dev/null || fail "setup needed again after $1"
  docker compose exec -T fvoci /opt/fvoci/bin/fvoci-migrate --verify-secrets >/dev/null || fail "sealed secrets after $1"
}

step "second up -d, restart (preparation runs again), recreate and down/up keep data and keys"
docker compose up -d
wait_healthy
docker compose restart fvoci
wait_healthy
[[ "$(logs fvoci | grep -c 'prepared; starting the server')" -ge 2 ]] || fail "restart did not prepare again"
[[ "$(logs fvoci | grep -c 'created app role')" == 1 ]] || fail "app role not created exactly once"
check_same_install "restart"
docker compose up -d --force-recreate
wait_healthy
check_same_install "up -d --force-recreate"
# SIGTERM reaches the server as pid 1 after the uid drop: graceful exit 0.
docker compose stop fvoci
[[ "$(state fvoci)" == "exited 0" ]] || fail "server did not stop gracefully: $(state fvoci)"
docker compose down
docker compose up -d
wait_healthy
check_same_install "down/up"
echo "same-version re-runs and recreated containers: role created once, login (same pepper), document, sealed secrets: ok"

step "no secret value in any container log"
LOGS="$(docker compose logs --no-color 2>&1)"
N=0
for key in POSTGRES_PASSWORD FVOCI_APP_PASSWORD MEILI_MASTER_KEY PASSWORD_PEPPER_KEYS ENCRYPTION_KEYS; do
  value="$(env_value "$key")"
  secret="${value#*\":\"}"; secret="${secret%\"\}}"
  grep -qF "$secret" <<<"$LOGS" && fail "$key appears in container logs"
  N=$((N + 1))
done
echo "checked $N secret values against $(wc -l <<<"$LOGS") log lines: none found"

step "a changed POSTGRES_PASSWORD: restart keeps the old value, up -d applies it and is refused, the original starts again"
set_env POSTGRES_PASSWORD "$(smoke_ts random-hex)"
# The container keeps the environment it was created with.
docker compose restart fvoci
wait_healthy
check_same_install "restart after editing .env"
# up -d recreates every container whose configuration changed (fvoci and
# postgres); PostgreSQL keeps the password from its first start.
docker compose up -d
wait_refused 'POSTGRES_PASSWORD is not the password of fvoci_owner'
set_env POSTGRES_PASSWORD "$OWNER_PW"
docker compose up -d
wait_healthy
check_same_install "restoring POSTGRES_PASSWORD"
echo "changed owner password: kept out by restart, named and refused after up -d; original value recovers: ok"

step "a preparation failure (grants refused by a read-only database) keeps the server down"
psql_owner 'ALTER DATABASE fvoci SET default_transaction_read_only = on'
docker compose restart fvoci
wait_refused 'cannot execute GRANT in a read-only transaction'
psql_owner 'ALTER DATABASE fvoci RESET default_transaction_read_only'
docker compose restart fvoci
wait_healthy
echo "failed preparation: not healthy, /ready refused; after the fix the restart recovers: ok"

step "pending migrations while the server is live: preparation refuses"
LATEST="$(psql_owner 'SELECT max(version) FROM fvoci.schema_migrations' fvoci)"
# The receipt carries lineage and digest; hold the exact row aside instead of re-inserting a bare version.
psql_owner "CREATE TABLE public.smoke_held_receipt AS SELECT * FROM fvoci.schema_migrations WHERE version = ${LATEST}" fvoci
psql_owner "DELETE FROM fvoci.schema_migrations WHERE version = ${LATEST}" fvoci
set +e
OUT="$(docker compose run --rm --no-deps -T --entrypoint /opt/fvoci/bin/fvoci-migrate fvoci --prepare 2>&1)"
STATUS=$?
set -e
psql_owner "INSERT INTO fvoci.schema_migrations SELECT * FROM public.smoke_held_receipt; DROP TABLE public.smoke_held_receipt" fvoci
grep 'migration(s) are pending' <<<"$OUT" | cut -c1-220
if (( STATUS == 0 )) || ! grep -q 'another FVOCI server is still running' <<<"$OUT"; then
  fail "live-writer upgrade not refused"
fi
echo "upgrade while a server holds app-role sessions: refused, pointing to RUNNING.md Upgrade: ok"

step "SIGTERM during the readiness wait and the readiness deadline"
docker compose stop fvoci postgres
docker start "$(docker compose ps -a -q fvoci)" >/dev/null
sleep 2
T0=$SECONDS
docker stop -t 20 "$(docker compose ps -a -q fvoci)" >/dev/null
STOP_SECS=$((SECONDS - T0))
[[ "$(state fvoci)" == "exited 143" ]] || fail "fvoci did not exit 143 on SIGTERM: $(state fvoci)"
(( STOP_SECS < 10 )) || fail "SIGTERM took ${STOP_SECS}s"
set +e
OUT="$(docker compose run --rm --no-deps -T -e FVOCI_PREPARE_TIMEOUT_SECS=3 fvoci 2>&1)"
STATUS=$?
set -e
grep 'not ready before the deadline' <<<"$OUT" | cut -c1-160
(( STATUS == 1 )) || fail "deadline exit $STATUS"
docker compose up -d
wait_healthy
echo "SIGTERM while waiting: exit 143 in ${STOP_SECS}s; 3 s deadline without PostgreSQL: exit 1: ok"

step "scripts/backup.sh and restore.sh with the same .env"
bash "$ROOT/scripts/backup.sh" --project "$MAIN" --env-file .env --compose-file compose.yml --output "$WORK/backup" | tail -1
docker compose down
RESTORED="${MAIN}-restored"
PROJECTS+=("$RESTORED")
bash "$ROOT/scripts/restore.sh" --project "$RESTORED" --env-file .env --compose-file compose.yml --input "$WORK/backup" | tail -1
export COMPOSE_PROJECT_NAME="$RESTORED"
check_same_install "restore"
docker compose down -v
export COMPOSE_PROJECT_NAME="$MAIN"
docker compose up -d
wait_healthy
echo "backup + restore into $RESTORED: login, document, sealed secrets: ok"

step "concurrent double up -d on a fresh install"
docker compose down -v
export COMPOSE_PROJECT_NAME="${MAIN}-race"
PROJECTS+=("$COMPOSE_PROJECT_NAME")
set +e
docker compose up -d >"$WORK/race1.log" 2>&1 &
P1=$!
docker compose up -d >"$WORK/race2.log" 2>&1 &
P2=$!
wait "$P1"; S1=$?
wait "$P2"; S2=$?
set -e
echo "concurrent up -d exit codes: $S1 $S2"
docker compose up -d
wait_healthy
[[ "$(logs fvoci | grep -c 'created app role')" -le 1 ]] || fail "app role created twice"
check_same_install_fresh() {
  curl -fsS -c "$JAR" -b "$JAR" -H 'content-type: application/json' -H "origin: $ORIGIN" -X POST "$BASE/api/v1/setup" \
    -d '{"email":"owner@install.test","password":"installpass1","givenName":"Owner","workspaceSlug":"race","workspaceName":"Race"}' >/dev/null
  rm -f "$JAR"
  login
}
check_same_install_fresh
echo "double up -d: one preparation, setup + login: ok"

step "two preparations at once on a fresh database"
docker compose down -v
docker compose up -d --wait postgres meilisearch
set +e
docker compose run --rm --no-deps -T --entrypoint /opt/fvoci/bin/fvoci-migrate fvoci --prepare >"$WORK/prep1.log" 2>&1 &
P1=$!
docker compose run --rm --no-deps -T --entrypoint /opt/fvoci/bin/fvoci-migrate fvoci --prepare >"$WORK/prep2.log" 2>&1 &
P2=$!
wait "$P1"; S1=$?
wait "$P2"; S2=$?
set -e
(( S1 == 0 && S2 == 0 )) || fail "concurrent preparations exited $S1 $S2: $(cat "$WORK/prep1.log" "$WORK/prep2.log")"
[[ "$(cat "$WORK/prep1.log" "$WORK/prep2.log" | grep -c 'created app role')" == 1 ]] || fail "app role not created exactly once"
echo "concurrent preparations: both exit 0, app role created once: ok"

step "install smoke passed"
