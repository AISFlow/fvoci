#!/usr/bin/env bash
# Restore a logical backup into a FRESH infra/rust Compose project.
#
# Target volumes must not already exist. Restores PostgreSQL, restores the
# storage volume, then starts init (fvoci-migrate, --grant-app-role,
# --ensure-meili-key), checks every stored attachment exists in storage
# (fvoci-migrate --verify-storage) and starts the server. Meilisearch data is not in the backup;
# init creates a scoped key and empty index with the required settings.
#
# Keep POSTGRES_USER, POSTGRES_DB, and FVOCI_APP_ROLE names the same as the
# backed-up install. Database and Meili passwords may be new. PASSWORD_PEPPER_KEYS
# must match the original or existing passwords will not verify. ENCRYPTION_KEYS
# must hold every key id of the original with the same key (a rotated superset
# is fine); both are checked against the manifest before any volume exists.
# After the database is restored, fvoci-migrate --verify-secrets opens every
# sealed secret (MFA, workspace SSO, webhooks) before the server starts.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="$ROOT/infra/rust/compose.yml"
PROJECT=""
ENV_FILE=""
INPUT=""
TAR_IMAGE="postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db"
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

read_env() {
  local key="$1"
  local line
  line="$(grep -E "^${key}=" "$ENV_FILE" || true)"
  if [[ -z "$line" ]]; then
    echo "missing ${key} in env file" >&2
    exit 1
  fi
  printf '%s\n' "${line#*=}"
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
SELECTED_IMAGE="$(jq -er '.services.server.image' <<<"$COMPOSE_CONFIG")"
PRODUCT_IMAGE_ID="$(docker image inspect -f '{{.Id}}' "$SELECTED_IMAGE")"
compose_key() {
  jq -r --arg name "$1" '.services.server.environment[$name] // ""' <<<"$COMPOSE_CONFIG"
}
PEPPER_KEYS="$(compose_key PASSWORD_PEPPER_KEYS)"
PEPPER_ACTIVE="$(compose_key PASSWORD_PEPPER_ACTIVE_KEY_ID)"
ENCRYPTION_KEYS_VALUE="$(compose_key ENCRYPTION_KEYS)"
ENCRYPTION_ACTIVE="$(compose_key ENCRYPTION_ACTIVE_KEY_ID)"
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

APP_ROLE="$(read_env FVOCI_APP_ROLE)"
APP_PASSWORD="$(read_env FVOCI_APP_PASSWORD)"
if [[ ! "$APP_ROLE" =~ ^[a-zA-Z_][a-zA-Z0-9_]*$ ]]; then
  echo "invalid FVOCI_APP_ROLE" >&2
  exit 1
fi

echo "creating empty restore stack (no processes started)"
"${COMPOSE[@]}" up --no-start

SERVER_CID="$("${COMPOSE[@]}" ps -a -q server | head -1)"
if [[ -z "$SERVER_CID" ]]; then
  echo "restore server container was not created" >&2
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
"${COMPOSE[@]}" exec -T \
  -e app_role="$APP_ROLE" \
  -e app_password="$APP_PASSWORD" \
  postgres \
  sh -c 'exec psql -X -v ON_ERROR_STOP=1 -v app_role="$app_role" -v app_password="$app_password" -U "$POSTGRES_USER" -d "$POSTGRES_DB"' <<'SQL'
SELECT format('CREATE ROLE %I LOGIN PASSWORD %L NOSUPERUSER NOBYPASSRLS', :'app_role', :'app_password')
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = :'app_role')
\gexec
SQL

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
"${COMPOSE[@]}" run --rm init

# Event xids from the old cluster are not comparable with this one; rebase the
# outbox cursors before any server starts (writers are still stopped here).
# createdAt is taken after the quiesced dump but has whole-second precision, so
# events from earlier in that same second sort after it; use the next second.
echo "rebasing outbox cursors (snapshot $SNAPSHOT_AT)"
"${COMPOSE[@]}" run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate init \
  --recover-outbox --since "$SINCE" --snapshot-at "$SNAPSHOT_AT" \
  --apply --reason "restore into $PROJECT" --ack-external-replay

echo "rebuilding the search index from PostgreSQL"
"${COMPOSE[@]}" run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate init --rebuild-search

# Every stored attachment in the restored database must exist in the storage
# the server will use, with its recorded size. Runs with the server's own
# environment (app role, storage variables), before the server starts.
echo "verifying stored attachments against the configured storage"
"${COMPOSE[@]}" run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate server --verify-storage

# Every sealed secret in the restored database must open with the server's
# ENCRYPTION_KEYS (same environment and app role as the server).
echo "opening every sealed secret with the configured ENCRYPTION_KEYS"
"${COMPOSE[@]}" run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate server --verify-secrets

echo "starting the server"
"${COMPOSE[@]}" up -d --wait server

jq -nc --arg project "$PROJECT" '{restoredProject: $project, searchRebuilt: "rebuild-search", storageVerified: true, secretsVerified: true}'
