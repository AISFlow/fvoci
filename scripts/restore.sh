#!/usr/bin/env bash
# Restore a logical backup into a FRESH infra/rust Compose project whose
# volumes do not exist yet, in this order:
#   1. offline preflight of the backup against the manifest (format,
#      PostgreSQL major version, file hashes, dump magic and tar layout,
#      source differs from target, pepper and ENCRYPTION_KEYS fingerprints):
#      fvoci-migrate --restore-preflight in the product image with no network,
#      before any target volume exists;
#   2. storage volume restore, then the app role and pg_restore;
#   3. preparation: the developer stack's init service (fvoci-migrate,
#      --grant-app-role, --ensure-meili-key), or `fvoci-migrate --prepare` in
#      the app service of the user install (compose.user.yml has no init);
#   4. --recover-outbox: rebase the outbox cursors onto this cluster's xids;
#   5. --rebuild-search: reindex from PostgreSQL (Meilisearch data is not in
#      the backup);
#   6. --verify-storage: every stored attachment and published preview exists
#      with its recorded size, and every branding asset with its digest;
#   7. --verify-secrets: every sealed secret (MFA, workspace SSO, webhooks, the
#      VAPID key) opens with the configured ENCRYPTION_KEYS;
#   8. server start.
#
# Keep POSTGRES_USER, POSTGRES_DB, and FVOCI_APP_ROLE names the same as the
# backed-up install. Database and Meili passwords may be new. PASSWORD_PEPPER_KEYS
# must match the original or existing passwords will not verify. ENCRYPTION_KEYS
# must hold every key id of the original with the same key (a rotated superset
# is fine); step 1 checks both.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="$ROOT/infra/rust/compose.yml"
PROJECT=""
ENV_FILE=""
INPUT=""
TAR_IMAGE="ghcr.io/aisflow/fvoci/ci/postgres@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db"
VOLUME_KEYS=(pgdata storage searchdata meili_key)

usage() {
  cat <<'EOF' >&2
usage: scripts/restore.sh --project NAME --env-file PATH --input DIR [options]

  --project NAME       New Compose project (must differ from the backup source)
  --env-file PATH      Env for the restore stack (same pepper as the original)
  --input DIR          Backup directory from scripts/backup.sh
  --compose-file PATH  default: infra/rust/compose.yml

The named project must have none of the install volumes yet. On failure the
target stack is left in place for diagnosis; the operator deletes only that
project (docker compose -p NAME down -v).

EOF
  exit 2
}

# --- env-file values (checked by scripts/test-restore-env.sh) ---
# Read KEY from the env file the way docker compose does for the values an
# install uses: KEY=value, KEY='value' or KEY="value". Anything whose Compose
# meaning could differ from the literal text (escapes, $ interpolation, inline
# comments, whitespace, export, a repeated key) is refused, not guessed.
env_file_value() {
  local key="$1" file="$2" line value
  local -a lines=()
  mapfile -t lines < <(grep -E "^[[:space:]]*(export[[:space:]]+)?${key}[[:space:]]*=" "$file" || true)
  if ((${#lines[@]} == 0)); then
    return 1
  fi
  if ((${#lines[@]} > 1)); then
    echo "${key} is set more than once in the env file" >&2
    return 2
  fi
  line="${lines[0]%$'\r'}"
  if [[ "$line" != "${key}="* ]]; then
    echo "${key} in the env file must be written as ${key}=value (no export or spaces)" >&2
    return 2
  fi
  value="${line#*=}"
  if [[ "$value" =~ ^\'([^\']*)\'$ ]]; then
    value="${BASH_REMATCH[1]}"
  elif [[ "$value" =~ ^\"([^\"\\\$]*)\"$ ]]; then
    value="${BASH_REMATCH[1]}"
  elif [[ "$value" =~ ^[\'\"] || "$value" =~ [[:space:]#\$\\] ]]; then
    echo "${key} in the env file must be unquoted, or fully in single or double quotes without \\, \$ or inner quotes" >&2
    return 2
  fi
  printf '%s\n' "$value"
}
# --- end env-file values ---

# --- app role (checked by scripts/test-restore-env.sh) ---
# The dump grants to the app role, so it must exist before pg_restore. The
# password stays off every argv, which any local user can read from /proc:
# docker/compose on the host and psql in the container. It reaches compose's
# environment for this one command, `-e app_password` (a name, no value) copies
# it into the exec environment, and psql reads it with \getenv. If it is
# missing there, :'app_password' stays unexpanded and ON_ERROR_STOP aborts.
# The CREATE ROLE ... PASSWORD statement itself still reaches the server log
# when log_statement is ddl or all.
# shellcheck disable=SC2016 # the sh -c body expands in the postgres container
create_app_role() {
  app_password="$APP_PASSWORD" "${COMPOSE[@]}" exec -T \
    -e app_role="$APP_ROLE" \
    -e app_password \
    postgres \
    sh -c 'exec psql -X -v ON_ERROR_STOP=1 -v app_role="$app_role" -U "$POSTGRES_USER" -d "$POSTGRES_DB"' <<'SQL'
\getenv app_password app_password
SELECT format('CREATE ROLE %I LOGIN PASSWORD %L NOSUPERUSER NOBYPASSRLS', :'app_role', :'app_password')
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = :'app_role')
\gexec
SQL
}
# --- end app role ---

read_env() {
  local key="$1" default="${2-}" status=0
  env_file_value "$key" "$ENV_FILE" || status=$?
  if ((status == 1)) && [[ -n "$default" ]]; then
    printf '%s\n' "$default"
    return
  fi
  if ((status == 1)); then
    echo "missing ${key} in env file" >&2
    exit 1
  fi
  ((status == 0)) || exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --project)
      PROJECT="${2:-}"
      shift 2
      ;;
    --env-file)
      ENV_FILE="${2:-}"
      shift 2
      ;;
    --input)
      INPUT="${2:-}"
      shift 2
      ;;
    --compose-file)
      COMPOSE_FILE="${2:-}"
      shift 2
      ;;
    -h | --help)
      usage
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage
      ;;
  esac
done

[[ -n "$PROJECT" && -n "$ENV_FILE" && -n "$INPUT" ]] || usage
# Manifest/key checks run in the selected product image before any target volume exists.
for dependency in docker jq; do
  command -v "$dependency" >/dev/null 2>&1 || {
    echo "restore host requires $dependency" >&2
    exit 1
  }
done
docker compose version >/dev/null
if [[ ! "$PROJECT" =~ ^[a-z0-9][a-z0-9_-]{0,62}$ ]]; then
  echo "--project must be a lowercase Compose project name" >&2
  exit 1
fi
if [[ ! -f "$ENV_FILE" ]]; then
  echo "env file not found: $ENV_FILE" >&2
  exit 1
fi
if [[ ! -f "$COMPOSE_FILE" ]]; then
  echo "compose file not found: $COMPOSE_FILE" >&2
  exit 1
fi
if [[ "$INPUT" != /* ]]; then
  INPUT="$(pwd)/$INPUT"
fi
if [[ ! -d "$INPUT" ]]; then
  echo "backup directory not found: $INPUT" >&2
  exit 1
fi

DUMP="$INPUT/database.dump"
STORAGE_TAR="$INPUT/storage.tar"
MANIFEST="$INPUT/manifest.json"
for path in "$DUMP" "$STORAGE_TAR" "$MANIFEST"; do
  if [[ ! -s "$path" ]]; then
    echo "backup file missing or empty: $path" >&2
    exit 1
  fi
done

# Resolve the same product image as Compose will use for the target server.
# The offline check needs no DB, migration owner, network, or writeable backup.
COMPOSE=(docker compose -f "$COMPOSE_FILE" --project-name "$PROJECT" --env-file "$ENV_FILE")
COMPOSE_CONFIG="$("${COMPOSE[@]}" config --format json)"
# The app service publishes container port 8080; init is the one-shot service it
# waits for (`server`/`init` in compose.yml). compose.user.yml has no init: the
# app service's own fvoci-migrate runs those steps.
SERVER="$(jq -er '[.services | to_entries[] | select(any(.value.ports[]?; .target == 8080)) | .key]
  | if length == 1 then .[0] else error("expected one service publishing 8080") end' <<<"$COMPOSE_CONFIG")"
INIT="$(jq -r --arg s "$SERVER" '[.services[$s].depends_on // {} | to_entries[]
  | select(.value.condition == "service_completed_successfully") | .key] | .[0] // ""' <<<"$COMPOSE_CONFIG")"
PREP="${INIT:-$SERVER}"
compose_key() {
  jq -r --arg s "$SERVER" --arg name "$1" '.services[$s].environment[$name] // ""' <<<"$COMPOSE_CONFIG"
}
PEPPER_KEYS="$(compose_key PASSWORD_PEPPER_KEYS)"
if [[ -z "$PEPPER_KEYS" ]]; then
  # A compose.yml of an older release that passed the keyrings as secret files.
  echo "$COMPOSE_FILE passes no PASSWORD_PEPPER_KEYS to $SERVER (a release that used secret files); run scripts/restore.sh from that release" >&2
  exit 1
fi
SELECTED_IMAGE="$(jq -er --arg s "$SERVER" '.services[$s].image' <<<"$COMPOSE_CONFIG")"
PRODUCT_IMAGE_ID="$(docker image inspect -f '{{.Id}}' "$SELECTED_IMAGE")"
PEPPER_ACTIVE="$(compose_key PASSWORD_PEPPER_ACTIVE_KEY_ID)"
ENCRYPTION_KEYS_VALUE="$(compose_key ENCRYPTION_KEYS)"
ENCRYPTION_ACTIVE="$(compose_key ENCRYPTION_ACTIVE_KEY_ID)"
# These exported values take precedence over later env-file interpolation.
# Preflight and every subsequent Compose operation share this exact snapshot.
export FVOCI_IMAGE="$PRODUCT_IMAGE_ID"
export PASSWORD_PEPPER_KEYS="$PEPPER_KEYS" PASSWORD_PEPPER_ACTIVE_KEY_ID="$PEPPER_ACTIVE"
export ENCRYPTION_KEYS="$ENCRYPTION_KEYS_VALUE" ENCRYPTION_ACTIVE_KEY_ID="$ENCRYPTION_ACTIVE"
# A release compose pins the image instead of reading FVOCI_IMAGE: compare ids.
PINNED_CONFIG="$("${COMPOSE[@]}" config --format json)"
for service in "$SERVER" "$PREP"; do
  image="$(jq -er --arg s "$service" '.services[$s].image' <<<"$PINNED_CONFIG")"
  if [[ "$(docker image inspect -f '{{.Id}}' "$image")" != "$PRODUCT_IMAGE_ID" ]]; then
    echo "Compose must preserve the selected product image" >&2
    exit 1
  fi
done
# Each key the server environment gets is the exported value.
if ! jq -e --arg s "$SERVER" '(.services[$s].environment // {}) as $settings |
  all(["PASSWORD_PEPPER_KEYS", "PASSWORD_PEPPER_ACTIVE_KEY_ID", "ENCRYPTION_KEYS", "ENCRYPTION_ACTIVE_KEY_ID"][];
    . as $key | $settings[$key] == env[$key])' <<<"$PINNED_CONFIG" >/dev/null; then
  echo "Compose must preserve the selected product image and key snapshot" >&2
  exit 1
fi
PREFLIGHT="$(PASSWORD_PEPPER_KEYS="$PEPPER_KEYS" PASSWORD_PEPPER_ACTIVE_KEY_ID="$PEPPER_ACTIVE" \
  ENCRYPTION_KEYS="$ENCRYPTION_KEYS_VALUE" ENCRYPTION_ACTIVE_KEY_ID="$ENCRYPTION_ACTIVE" \
  docker run --rm --network none --read-only --user "$(id -u):$(id -g)" \
    --entrypoint /opt/fvoci/bin/fvoci-migrate \
    -e PASSWORD_PEPPER_KEYS -e PASSWORD_PEPPER_ACTIVE_KEY_ID \
    -e ENCRYPTION_KEYS -e ENCRYPTION_ACTIVE_KEY_ID \
    -v "${INPUT}:/backup:ro" "$PRODUCT_IMAGE_ID" \
    --restore-preflight /backup/manifest.json /backup/database.dump \
    /backup/storage.tar "$PROJECT")"
if [[ ! "$PREFLIGHT" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z\ [0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$ ]]; then
  echo "invalid restore preflight output" >&2
  exit 1
fi
read -r SNAPSHOT_AT SINCE <<<"$PREFLIGHT"

for vol in "${VOLUME_KEYS[@]}"; do
  if docker volume inspect "${PROJECT}_${vol}" >/dev/null 2>&1; then
    echo "volume ${PROJECT}_${vol} already exists; restore only into a fresh project" >&2
    exit 1
  fi
done

APP_ROLE="$(read_env FVOCI_APP_ROLE fvoci_app)"
APP_PASSWORD="$(read_env FVOCI_APP_PASSWORD)"
if [[ ! "$APP_ROLE" =~ ^[a-zA-Z_][a-zA-Z0-9_]*$ ]]; then
  echo "invalid FVOCI_APP_ROLE" >&2
  exit 1
fi

echo "creating empty restore stack (no processes started)"
"${COMPOSE[@]}" up --no-start

SERVER_CID="$("${COMPOSE[@]}" ps -a -q "$SERVER" | head -1)"
if [[ -z "$SERVER_CID" ]]; then
  echo "restore server container was not created" >&2
  exit 1
fi
INIT_CID="$("${COMPOSE[@]}" ps -a -q "$PREP" | head -1)"
if [[ -z "$INIT_CID" ]] ||
   [[ "$(docker inspect -f '{{.Image}}' "$SERVER_CID")" != "$PRODUCT_IMAGE_ID" ]] ||
   [[ "$(docker inspect -f '{{.Image}}' "$INIT_CID")" != "$PRODUCT_IMAGE_ID" ]]; then
  echo "created restore containers differ from the verified product image" >&2
  exit 1
fi
STORAGE_VOL="$(docker inspect -f '{{range .Mounts}}{{if eq .Destination "/data/storage"}}{{.Name}}{{end}}{{end}}' "$SERVER_CID")"
if [[ -z "$STORAGE_VOL" ]]; then
  echo "restore server container has no /data/storage volume" >&2
  exit 1
fi

echo "restoring storage volume"
docker run --rm --network none --user 0:0 --entrypoint tar \
  -v "${STORAGE_VOL}:/v" \
  -v "${INPUT}:/b:ro" \
  "$TAR_IMAGE" \
  --numeric-owner -xf /b/storage.tar -C /v

echo "starting postgres and meilisearch"
"${COMPOSE[@]}" up -d --wait postgres meilisearch

PG_CID="$("${COMPOSE[@]}" ps -q postgres)"
if [[ -z "$PG_CID" ]]; then
  echo "postgres did not start" >&2
  exit 1
fi

RELATIONS="$("${COMPOSE[@]}" exec -T postgres sh -c \
  'psql -X -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" -tAc "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind IN ('\''r'\'','\''p'\'','\''v'\'','\''m'\'','\''S'\'','\''f'\'') AND n.nspname NOT IN ('\''pg_catalog'\'','\''information_schema'\'') AND n.nspname !~ '\''^pg_toast'\'' AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.classid='\''pg_class'\''::regclass AND d.objid=c.oid AND d.deptype='\''e'\'')"')"
RELATIONS="$(printf '%s' "$RELATIONS" | tr -d '[:space:]')"
if [[ "$RELATIONS" != "0" ]]; then
  echo "restore PostgreSQL target is not empty (${RELATIONS} user relations)" >&2
  exit 1
fi

echo "creating application role ${APP_ROLE}"
create_app_role

echo "restoring PostgreSQL dump"
docker cp "$DUMP" "${PG_CID}:/tmp/fvoci-restore.dump"
# Fresh clusters already have schema public; skip recreating it so
# --single-transaction restore can load public helper functions.
"${COMPOSE[@]}" exec -T postgres sh -c \
  'pg_restore --list /tmp/fvoci-restore.dump | grep -v " SCHEMA - public " >/tmp/fvoci-restore.list'
"${COMPOSE[@]}" exec -T postgres sh -c \
  'pg_restore -U "$POSTGRES_USER" -d "$POSTGRES_DB" --exit-on-error --single-transaction --no-owner --use-list=/tmp/fvoci-restore.list /tmp/fvoci-restore.dump'
"${COMPOSE[@]}" exec -T postgres rm -f /tmp/fvoci-restore.dump /tmp/fvoci-restore.list

echo "running migrate, grant-app-role, and ensure-meili-key"
if [[ -n "$INIT" ]]; then
  "${COMPOSE[@]}" run --rm "$INIT"
else
  "${COMPOSE[@]}" run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate "$SERVER" --prepare
fi

# Event xids from the old cluster are not comparable with this one; rebase the
# outbox cursors before any server starts (writers are still stopped here).
# createdAt is taken after the quiesced dump but has whole-second precision, so
# events from earlier in that same second sort after it; use the next second.
echo "rebasing outbox cursors (snapshot $SNAPSHOT_AT)"
"${COMPOSE[@]}" run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate "$PREP" \
  --recover-outbox --since "$SINCE" --snapshot-at "$SNAPSHOT_AT" \
  --apply --reason "restore into $PROJECT" --ack-external-replay

echo "rebuilding the search index from PostgreSQL"
"${COMPOSE[@]}" run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate "$PREP" --rebuild-search

# Every stored attachment in the restored database must exist in the storage
# the server will use, with its recorded size. Runs with the server's own
# environment (app role, storage variables), before the server starts.
echo "verifying stored attachments against the configured storage"
"${COMPOSE[@]}" run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate "$SERVER" --verify-storage

# Every sealed secret in the restored database must open with the server's
# ENCRYPTION_KEYS (same environment and app role as the server).
echo "opening every sealed secret with the configured ENCRYPTION_KEYS"
"${COMPOSE[@]}" run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate "$SERVER" --verify-secrets

echo "starting the server"
"${COMPOSE[@]}" up -d --wait "$SERVER"

jq -nc --arg project "$PROJECT" '{restoredProject: $project, searchRebuilt: "rebuild-search", storageVerified: true, secretsVerified: true}'
