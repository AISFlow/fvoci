#!/usr/bin/env bash
# Logical backup of a running infra/rust Compose install.
#
# Stops the server (the only writer) so PostgreSQL and attachment files are
# taken from the same quiesced moment: stored attachment keys in the dump are a
# subset of objects in the storage volume. Dumps schemas public (RLS helpers)
# and fvoci in pg_dump custom format as the owner role, then archives the
# storage volume.
#
# Meilisearch data is not included. The index is derived; restore recreates a
# scoped key and index settings. Product search-rebuild is not in this slice.
# Pepper keys, ENCRYPTION_KEYS, DB passwords, and the Meili master key stay in
# the operator env file — they are not copied into the archive (beyond whatever
# the database dump already contains). The manifest records only fingerprints
# of the pepper keyring and of each ENCRYPTION_KEYS key id, which restore
# compares before touching volumes.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="$ROOT/infra/rust/compose.yml"
LEAVE_STOPPED=0
PROJECT=""
ENV_FILE=""
OUTPUT=""
# Same image the compose stack already pulled (has tar); run as root so the
# host bind mount is writable. Numeric owners in the archive stay uid 1000.
TAR_IMAGE="postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db"

usage() {
  cat <<'EOF' >&2
usage: scripts/backup.sh --project NAME --env-file PATH --output DIR [options]

  --project NAME       Compose project name of the running install
  --env-file PATH      Compose env file (not copied into the archive)
  --output DIR         New directory for the backup (mode 0700); must not exist
  --compose-file PATH  default: infra/rust/compose.yml
  --leave-stopped      do not restart the server after the dump

EOF
  exit 2
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
    --output)
      OUTPUT="${2:-}"
      shift 2
      ;;
    --compose-file)
      COMPOSE_FILE="${2:-}"
      shift 2
      ;;
    --leave-stopped)
      LEAVE_STOPPED=1
      shift
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

[[ -n "$PROJECT" && -n "$ENV_FILE" && -n "$OUTPUT" ]] || usage
# These are host-side tools; no Python runtime is needed for these operations.
# Fail before stopping the server or creating backup state.
for dependency in docker jq tar; do
  command -v "$dependency" >/dev/null 2>&1 || {
    echo "backup host requires $dependency" >&2
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
if [[ "$OUTPUT" != /* ]]; then
  OUTPUT="$(pwd)/$OUTPUT"
fi
if [[ -e "$OUTPUT" ]]; then
  echo "backup output already exists: $OUTPUT" >&2
  exit 1
fi
PARENT="$(dirname "$OUTPUT")"
if [[ ! -d "$PARENT" ]]; then
  echo "backup parent directory does not exist: $PARENT" >&2
  exit 1
fi

COMPOSE=(docker compose -f "$COMPOSE_FILE" --project-name "$PROJECT" --env-file "$ENV_FILE")
STAGING="${OUTPUT}.partial-$$"
SERVER_STOPPED=0

cleanup() {
  local status=$?
  if [[ -d "$STAGING" ]]; then
    rm -rf "$STAGING"
  fi
  if (( SERVER_STOPPED == 1 && LEAVE_STOPPED == 0 )); then
    "${COMPOSE[@]}" up -d --wait server >/dev/null 2>&1 || true
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

mkdir -m 700 "$STAGING"

PG_CID="$("${COMPOSE[@]}" ps -q postgres)"
if [[ -z "$PG_CID" ]]; then
  echo "postgres service is not running for project ${PROJECT}" >&2
  exit 1
fi
SERVER_CID="$("${COMPOSE[@]}" ps -a -q server | head -1)"
if [[ -z "$SERVER_CID" ]]; then
  echo "server container is missing for project ${PROJECT}" >&2
  exit 1
fi

# The storage archive below is the local driver's volume. With S3 the objects
# live in the bucket; archiving the (unused) volume would claim a backup that
# cannot restore them. See RUNNING.md "S3 storage backup".
# The server trims the value; match case-insensitively with surrounding
# whitespace so no spelling the server might accept slips past this guard.
if docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$SERVER_CID" \
  | grep -Eqi '^[[:space:]]*STORAGE_DRIVER[[:space:]]*=[[:space:]]*s3[[:space:]]*$'; then
  echo "server uses STORAGE_DRIVER=s3: this script archives the local storage volume and cannot back up bucket objects." >&2
  echo "Back up PostgreSQL with pg_dump and protect the bucket with versioning/replication (RUNNING.md, S3 storage backup)." >&2
  exit 1
fi

STORAGE_VOL="$(docker inspect -f '{{range .Mounts}}{{if eq .Destination "/data/storage"}}{{.Name}}{{end}}{{end}}' "$SERVER_CID")"
if [[ -z "$STORAGE_VOL" ]]; then
  echo "server container has no /data/storage volume" >&2
  exit 1
fi
docker volume inspect "$STORAGE_VOL" >/dev/null
# Use the exact installed product image that is running this server. No pull or
# alternate host executable may decide the backup key/manifest policy.
SELECTED_IMAGE="$("${COMPOSE[@]}" config --format json | jq -er '.services.server.image')"
PRODUCT_IMAGE_ID="$(docker image inspect -f '{{.Id}}' "$SELECTED_IMAGE")"
if [[ "$(docker inspect -f '{{.Image}}' "$SERVER_CID")" != "$PRODUCT_IMAGE_ID" ]]; then
  echo "running server image differs from the selected Compose product image" >&2
  exit 1
fi
runtime_key() {
  local name="$1"
  docker inspect "$SERVER_CID" | jq -r --arg name "$name" \
    '.[0].Config.Env | map(select(startswith($name + "="))) | last | if . == null then "" else .[($name | length) + 1:] end'
}
PEPPER_KEYS="$(runtime_key PASSWORD_PEPPER_KEYS)"
PEPPER_ACTIVE="$(runtime_key PASSWORD_PEPPER_ACTIVE_KEY_ID)"
ENCRYPTION_KEYS_VALUE="$(runtime_key ENCRYPTION_KEYS)"
ENCRYPTION_ACTIVE="$(runtime_key ENCRYPTION_ACTIVE_KEY_ID)"

# Keep restart (including error cleanup) on the exact image and keys we backed
# up, even if the original tag or env file changes during the operation.
export FVOCI_IMAGE="$PRODUCT_IMAGE_ID"
export PASSWORD_PEPPER_KEYS="$PEPPER_KEYS" PASSWORD_PEPPER_ACTIVE_KEY_ID="$PEPPER_ACTIVE"
export ENCRYPTION_KEYS="$ENCRYPTION_KEYS_VALUE" ENCRYPTION_ACTIVE_KEY_ID="$ENCRYPTION_ACTIVE"
if ! "${COMPOSE[@]}" config --format json | jq -e '
  .services.server.image == env.FVOCI_IMAGE and
  (.services.server.environment as $settings |
    all(["PASSWORD_PEPPER_KEYS", "PASSWORD_PEPPER_ACTIVE_KEY_ID", "ENCRYPTION_KEYS", "ENCRYPTION_ACTIVE_KEY_ID"][];
      . as $key | $settings[$key] == env[$key]))' >/dev/null; then
  echo "Compose must preserve the selected product image and key snapshot" >&2
  exit 1
fi

echo "stopping server so dump and storage share a quiesced point"
"${COMPOSE[@]}" stop -t 45 server
SERVER_STOPPED=1
STOP_STATE="$(docker inspect -f '{{.State.Status}} {{.State.ExitCode}} {{.State.OOMKilled}}' "$SERVER_CID")"
if [[ "$STOP_STATE" != "exited 0 false" ]]; then
  echo "server stop expected 'exited 0 false', got '${STOP_STATE}'" >&2
  exit 1
fi

SESSIONS="$("${COMPOSE[@]}" exec -T postgres sh -c 'psql -X -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" -tAc "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND backend_type='\''client backend'\''"')"
SESSIONS="$(printf '%s' "$SESSIONS" | tr -d '[:space:]')"
if [[ "$SESSIONS" != "0" ]]; then
  echo "other database sessions remain after stopping the server: ${SESSIONS}" >&2
  exit 1
fi

DUMP="$STAGING/database.dump"
"${COMPOSE[@]}" exec -T postgres sh -c \
  'pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" --format=custom --no-owner --schema=public --schema=fvoci' \
  >"$DUMP"
chmod 600 "$DUMP"
if [[ ! -s "$DUMP" ]]; then
  echo "pg_dump produced an empty file" >&2
  exit 1
fi
MAGIC="$(head -c 5 "$DUMP" || true)"
if [[ "$MAGIC" != "PGDMP" ]]; then
  echo "pg_dump output is not PostgreSQL custom format" >&2
  exit 1
fi

docker run --rm --network none --user 0:0 --entrypoint tar \
  -v "${STORAGE_VOL}:/v:ro" \
  "$TAR_IMAGE" \
  --numeric-owner -cf - -C /v . >"$STAGING/storage.tar"
chmod 600 "$STAGING/storage.tar"
if [[ ! -s "$STAGING/storage.tar" ]]; then
  echo "storage archive is empty" >&2
  exit 1
fi

TAR_LIST="$(tar -tf "$STAGING/storage.tar" | sed 's|^\./||')"
MISSING=0
# Capture first so a failing psql fails the backup instead of an empty loop.
# Published previews (variants.preview.key) are never regenerated, so they
# must be in the archive too.
STORED_KEYS="$("${COMPOSE[@]}" exec -T postgres sh -c \
  'psql -X -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" -tAc "SELECT storage_key FROM fvoci.attachments WHERE status='\''stored'\'' UNION ALL SELECT variants->'\''preview'\''->>'\''key'\'' FROM fvoci.attachments WHERE status='\''stored'\'' AND jsonb_typeof(variants->'\''preview'\''->'\''key'\'')='\''string'\''"')"
while IFS= read -r key; do
  [[ -z "$key" ]] && continue
  if ! grep -Fqx "objects/${key}/payload" <<<"$TAR_LIST"; then
    echo "storage archive missing objects/${key}/payload referenced by PostgreSQL" >&2
    MISSING=1
  fi
done <<<"$STORED_KEYS"
if (( MISSING != 0 )); then
  exit 1
fi

CREATED_AT="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
PG_VERSION="$("${COMPOSE[@]}" exec -T postgres sh -c 'psql -X -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" -tAc "SHOW server_version_num"')"
PG_VERSION="$(printf '%s' "$PG_VERSION" | tr -d '[:space:]')"
PASSWORD_PEPPER_KEYS="$PEPPER_KEYS" PASSWORD_PEPPER_ACTIVE_KEY_ID="$PEPPER_ACTIVE" \
ENCRYPTION_KEYS="$ENCRYPTION_KEYS_VALUE" ENCRYPTION_ACTIVE_KEY_ID="$ENCRYPTION_ACTIVE" \
docker run --rm --network none --read-only --user "$(id -u):$(id -g)" \
  --entrypoint /opt/fvoci/bin/fvoci-migrate \
  -e PASSWORD_PEPPER_KEYS -e PASSWORD_PEPPER_ACTIVE_KEY_ID \
  -e ENCRYPTION_KEYS -e ENCRYPTION_ACTIVE_KEY_ID \
  -v "${STAGING}:/backup" "$PRODUCT_IMAGE_ID" \
  --backup-manifest /backup/manifest.json "$PROJECT" "$CREATED_AT" "$PG_VERSION" \
  /backup/database.dump /backup/storage.tar

mv --no-target-directory --no-clobber "$STAGING" "$OUTPUT"
chmod 700 "$OUTPUT"

if (( LEAVE_STOPPED == 0 )); then
  "${COMPOSE[@]}" up -d --wait server
  SERVER_STOPPED=0
fi

jq -nc --arg backup "$OUTPUT" '{backup: $backup, objectsChecked: true}'
