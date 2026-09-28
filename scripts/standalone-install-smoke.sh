#!/usr/bin/env bash
# Standalone install smoke: infra/rust/compose.user.yml alone in an empty
# folder, `docker compose up -d`, no env file. Checks secret bootstrap,
# first-admin setup/login, idempotent re-up, down/up persistence, per-audience
# secret isolation, and that data without its secret volumes refuses to start.
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
P="$COMPOSE_PROJECT_NAME"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-standalone.${RUN_ID}.XXXXXX")"
JAR="$WORK/.cookies"
BASE=http://localhost:8080
ORIGIN=http://localhost:8080
SECRET_VOLUMES=(secrets_postgres secrets_meilisearch secrets_init secrets_server)
SECRET_MOUNTS=()
for v in "${SECRET_VOLUMES[@]}"; do SECRET_MOUNTS+=(-v "${COMPOSE_PROJECT_NAME}_${v}:/s/${v}:ro"); done

step() { printf '== %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

cleanup() {
  local status=$?
  if (( status != 0 )); then
    (cd "$WORK" && docker compose ps -a >&2; docker compose logs --no-color --tail 80 >&2) || true
  fi
  (cd "$WORK" && docker compose down -v --remove-orphans >/dev/null 2>&1) || true
  for v in "${SECRET_VOLUMES[@]}" pgdata storage searchdata meili_key; do
    docker volume rm -f "${P}_${v}" >/dev/null 2>&1 || true
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

# All secret file contents, via a throwaway root reader (never printed).
secret_values() {
  docker run --rm --network none --user 0:0 --entrypoint sh \
    "${SECRET_MOUNTS[@]}" \
    "$IMAGE" -c 'for f in /s/*/*; do cat "$f"; echo; done'
}
# Name, mode, owner and sha256 of every file, markers included.
secret_manifest() {
  docker run --rm --network none --user 0:0 --entrypoint sh \
    "${SECRET_MOUNTS[@]}" \
    "$IMAGE" -c 'cd /s && for f in */* */.fvoci*; do
      printf "%s %s %s\n" "$(stat -c "%a %u:%g" "$f")" "$(sha256sum "$f" | cut -c1-64)" "$f"; done'
}
service_exit() { docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' "$(docker compose ps -a -q "$1")"; }
wait_healthy() {
  local deadline=$((SECONDS + 180)) cid
  while (( SECONDS < deadline )); do
    cid="$(docker compose ps -q server)"
    if [[ -n "$cid" && "$(docker inspect -f '{{.State.Health.Status}}' "$cid")" == healthy ]]; then
      return 0
    fi
    sleep 2
  done
  fail "server did not become healthy"
}
login() {
  curl -fsS -c "$JAR" -b "$JAR" -H 'content-type: application/json' -H "origin: $ORIGIN" \
    -X POST "$BASE/api/v1/auth/login" -d '{"email":"owner@standalone.test","password":"standalonepass1"}' \
    | jq -e '.userId' >/dev/null
}

step "first up -d (compose.yml only, no env file)"
T0=$SECONDS
docker compose up -d
wait_healthy
echo "healthy after $((SECONDS - T0))s"
[[ "$(service_exit bootstrap)" == "exited 0" ]] || fail "bootstrap: $(service_exit bootstrap)"
[[ "$(service_exit init)" == "exited 0" ]] || fail "init: $(service_exit init)"
docker compose logs --no-color bootstrap | grep -q "generated install secrets" || fail "bootstrap did not generate"
echo "bootstrap exited 0 (generated), init exited 0"

step "published ports"
docker compose ps --format json | jq -rs 'map(select(.Publishers != null) | .Service + " " + (.Publishers | map(select(.PublishedPort > 0) | "\(.URL):\(.PublishedPort)") | join(","))) | .[]'
PUBLISHED="$(docker compose ps --format json | jq -rs '[.[] | .Publishers[]? | select(.PublishedPort > 0) | "\(.URL):\(.PublishedPort)->\(.TargetPort)"] | unique | join(" ")')"
[[ "$PUBLISHED" == "127.0.0.1:8080->8080" ]] || fail "unexpected published ports: $PUBLISHED"

step "secret files: mode and owner"
MANIFEST1="$(secret_manifest)"
awk '{print $1, $2, $4}' <<<"$MANIFEST1"
awk '$4 !~ /\.fvoci-bootstrap-complete$/ && $1 != "600" {bad=1} END {exit bad}' <<<"$MANIFEST1" \
  || fail "secret file not 0600"
# Each audience's secret files belong to its reader (postgres 999, meilisearch
# 0, init and server fvoci 1000); markers are root's.
awk '
  $4 ~ /\.fvoci-bootstrap-complete$/ { if ($2 != "0:0") bad = bad " " $4; next }
  { want = "unexpected file" }
  $4 ~ /^secrets_postgres\//    { want = "999:999" }
  $4 ~ /^secrets_meilisearch\// { want = "0:0" }
  $4 ~ /^secrets_(init|server)\// { want = "1000:1000" }
  { if ($1 != "600" || $2 != want) bad = bad " " $4 }
  END { if (bad != "") { print "wrong mode/owner:" bad > "/dev/stderr"; exit 1 } }
' <<<"$MANIFEST1" || fail "secret file mode or owner"
[[ "$(awk '$4 ~ /\.fvoci-bootstrap-complete$/ {print $3}' <<<"$MANIFEST1" | sort -u | wc -l)" == 1 ]] \
  || fail "markers differ"
mapfile -t SECRETS < <(secret_values | awk 'length($0) >= 32')
(( ${#SECRETS[@]} >= 8 )) || fail "expected at least 8 long secret values, got ${#SECRETS[@]}"
OWNER_PW="$(docker run --rm --network none --user 0:0 -v "${P}_secrets_postgres:/s:ro" --entrypoint cat "$IMAGE" /s/postgres_password)"
MASTER_KEY="$(docker run --rm --network none --user 0:0 -v "${P}_secrets_meilisearch:/s:ro" --entrypoint cat "$IMAGE" /s/master_key)"

step "server cannot read the owner password or the Meilisearch master key"
docker compose exec -T server ls -A /run/fvoci/secrets
for path in /run/fvoci/secrets/postgres_password /run/fvoci/secrets/master_key \
  /run/fvoci/secrets/meili_master_key /run/fvoci/secrets/app_password; do
  if docker compose exec -T server cat "$path" >/dev/null 2>&1; then
    fail "server can read $path"
  fi
  echo "server: cat $path -> refused (absent)"
done
SERVER_VIEW="$(docker compose exec -T server sh -c 'cat /proc/1/environ | tr "\0" "\n"; for f in /run/fvoci/secrets/* /run/fvoci/meili/*; do cat "$f"; echo; done')"
grep -qF "$OWNER_PW" <<<"$SERVER_VIEW" && fail "owner password visible to server"
grep -qF "$MASTER_KEY" <<<"$SERVER_VIEW" && fail "master key visible to server"
docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$(docker compose ps -q server)" \
  | grep -E '^(POSTGRES_PASSWORD|MEILI_MASTER_KEY|DATABASE_URL|PASSWORD_PEPPER_KEYS|ENCRYPTION_KEYS)=' \
  && fail "secret value in server container config"
echo "owner password and master key not in server environ/secret mounts/config: ok"

step "first-admin setup and login over HTTP"
curl -fsS "$BASE/api/v1/setup" | jq -c .
curl -fsS -c "$JAR" -b "$JAR" -H 'content-type: application/json' -H "origin: $ORIGIN" \
  -X POST "$BASE/api/v1/setup" \
  -d '{"email":"owner@standalone.test","password":"standalonepass1","givenName":"Owner","workspaceSlug":"standalone","workspaceName":"Standalone"}' >/dev/null
login
WS="$(curl -fsS -b "$JAR" "$BASE/api/v1/me/workspaces" | jq -er '.items[0].id')"
DOC="$(curl -fsS -b "$JAR" -H 'content-type: application/json' -H "origin: $ORIGIN" \
  -X POST "$BASE/api/v1/workspaces/${WS}/documents" -d '{"parentId":null,"title":"Standalone doc"}' | jq -er .id)"
curl -fsS "$BASE/" | grep -qi '<!doctype html' || fail "React build not served at /"
echo "setup + login + document create + web root: ok"

step "second up -d leaves secrets byte-identical"
docker compose up -d
wait_healthy
docker compose logs --no-color bootstrap | grep -q "already present; nothing changed" || fail "bootstrap did not no-op"
[[ "$(secret_manifest)" == "$MANIFEST1" ]] || fail "secret files changed on second up"
echo "secret manifest identical after second up: ok"

step "down + up -d keeps data and keys"
docker compose down
docker compose up -d
wait_healthy
[[ "$(secret_manifest)" == "$MANIFEST1" ]] || fail "secret files changed after down/up"
rm -f "$JAR"
login
curl -fsS -b "$JAR" "$BASE/api/v1/me/workspaces" | jq -e --arg ws "$WS" '.items | map(.id) | index($ws) != null' >/dev/null \
  || fail "workspace lost"
curl -fsS -b "$JAR" "$BASE/api/v1/workspaces/${WS}/documents/${DOC}" | jq -e '.title == "Standalone doc"' >/dev/null \
  || fail "document lost"
curl -fsS "$BASE/api/v1/setup" | jq -c .
echo "login with the same pepper, workspace and document survive down/up: ok"

step "no secret value in any container log"
LOGS="$(docker compose logs --no-color 2>&1)"
for s in "${SECRETS[@]}"; do
  grep -qF "$s" <<<"$LOGS" && fail "a secret value appears in container logs"
done
echo "checked ${#SECRETS[@]} secret values against $(wc -l <<<"$LOGS") log lines: none found"

step "secret volumes removed while data exists: bootstrap refuses, server does not start"
docker compose down
for v in "${SECRET_VOLUMES[@]}"; do docker volume rm "${P}_${v}" >/dev/null; done
set +e
UP_OUT="$(docker compose up -d 2>&1)"
UP_STATUS=$?
set -e
printf '%s\n' "$UP_OUT" | tail -3
(( UP_STATUS != 0 )) || fail "up -d succeeded without secrets"
[[ "$(service_exit bootstrap)" == "exited 1" ]] || fail "bootstrap: $(service_exit bootstrap)"
docker compose logs --no-color bootstrap | grep "refusing to generate" | cut -c1-200
for svc in postgres meilisearch init server; do
  cid="$(docker compose ps -a -q "$svc")"
  if [[ -n "$cid" && "$(docker inspect -f '{{.State.StartedAt}}' "$cid")" != 0001-01-01T00:00:00Z ]]; then
    fail "$svc started without secrets"
  fi
done
for v in "${SECRET_VOLUMES[@]}"; do
  n="$(docker run --rm --network none -v "${P}_${v}:/s:ro" --entrypoint sh "$IMAGE" -c 'ls -A /s | wc -l')"
  [[ "$n" == 0 ]] || fail "bootstrap wrote into ${v} despite existing data"
done
echo "up -d exit ${UP_STATUS}; bootstrap exited 1; postgres/meilisearch/init/server never started; secret volumes left empty: ok"

step "standalone install smoke passed"
