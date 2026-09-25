#!/usr/bin/env bash
# Restore a logical backup into a FRESH infra/rust Compose project.
#
# Target volumes must not already exist. Restores PostgreSQL, restores the
# storage volume, then starts init (fvoci-migrate, --grant-app-role,
# --ensure-meili-key) and the server. Meilisearch data is not in the backup;
# init creates a scoped key and empty index with the required settings.
#
# Keep POSTGRES_USER, POSTGRES_DB, and FVOCI_APP_ROLE names the same as the
# backed-up install. Database and Meili passwords may be new. PASSWORD_PEPPER_KEYS
# must match the original or existing passwords will not verify.
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

pepper_fingerprint() {
  # SHA-256 over the canonical keyring JSON (sorted ids) and the active id. The
  # keys themselves never leave the env file.
  PEPPER_KEYS="$1" PEPPER_ACTIVE="$2" python3 -c '
import hashlib, json, os
ring = json.loads(os.environ["PEPPER_KEYS"])
if not isinstance(ring, dict) or not ring:
    raise SystemExit("PASSWORD_PEPPER_KEYS must be a non-empty JSON object")
canon = json.dumps({"keys": dict(sorted(ring.items())), "active": os.environ["PEPPER_ACTIVE"]}, separators=(",", ":"))
print(hashlib.sha256(canon.encode()).hexdigest())
'
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

python3 - "$MANIFEST" "$DUMP" "$STORAGE_TAR" "$PROJECT" <<'PY'
import hashlib, json, os, sys

manifest_path, dump_path, tar_path, project = sys.argv[1:5]
with open(manifest_path, encoding="utf-8") as fh:
    manifest = json.load(fh)
if manifest.get("formatVersion") != 1:
    raise SystemExit("unsupported backup format")
if manifest.get("schema") != "fvoci":
    raise SystemExit("backup schema is not fvoci")
source = manifest.get("sourceProject")
if not isinstance(source, str) or source == "":
    raise SystemExit("backup sourceProject is missing")
if source == project:
    raise SystemExit("restore target must use a different Compose project name")
search = manifest.get("search") or {}
if search.get("included") is True:
    raise SystemExit("this restore path does not accept archives that embed Meilisearch")


def check(entry, path):
    if not isinstance(entry, dict):
        raise SystemExit(f"invalid manifest entry for {path}")
    if entry.get("path") != os.path.basename(path):
        raise SystemExit(f"manifest path mismatch for {path}")
    size = os.path.getsize(path)
    digest = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            digest.update(chunk)
    sha = digest.hexdigest()
    if entry.get("sizeBytes") != size or entry.get("sha256") != sha:
        raise SystemExit(f"backup integrity check failed for {os.path.basename(path)}")


check(manifest.get("database"), dump_path)
check(manifest.get("storage"), tar_path)
PY

EXPECTED_PEPPER_FP="$(python3 -c 'import json,sys; print((json.load(open(sys.argv[1])).get("passwordPepper") or {}).get("fingerprint",""))' "$MANIFEST")"
if [[ -z "$EXPECTED_PEPPER_FP" ]]; then
  echo "backup manifest has no password pepper fingerprint; refusing to restore" >&2
  exit 1
fi
ACTUAL_PEPPER_FP="$(pepper_fingerprint "$(read_env PASSWORD_PEPPER_KEYS)" "$(read_env PASSWORD_PEPPER_ACTIVE_KEY_ID)")"
if [[ "$ACTUAL_PEPPER_FP" != "$EXPECTED_PEPPER_FP" ]]; then
  echo "PASSWORD_PEPPER_KEYS/ACTIVE_KEY_ID differ from the backed-up install; existing passwords could not be verified. Use the original keyring." >&2
  exit 1
fi

for vol in "${VOLUME_KEYS[@]}"; do
  if docker volume inspect "${PROJECT}_${vol}" >/dev/null 2>&1; then
    echo "volume ${PROJECT}_${vol} already exists; restore only into a fresh project" >&2
    exit 1
  fi
done

COMPOSE=(docker compose -f "$COMPOSE_FILE" --project-name "$PROJECT" --env-file "$ENV_FILE")
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
read -r SNAPSHOT_AT SINCE < <(python3 -c 'import datetime,json,sys; t=datetime.datetime.strptime(json.load(open(sys.argv[1]))["createdAt"],"%Y-%m-%dT%H:%M:%SZ"); f="%Y-%m-%dT%H:%M:%SZ"; print(t.strftime(f),(t-datetime.timedelta(days=29)).strftime(f))' "$MANIFEST")
echo "rebasing outbox cursors (snapshot $SNAPSHOT_AT)"
"${COMPOSE[@]}" run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate init \
  --recover-outbox --since "$SINCE" --snapshot-at "$SNAPSHOT_AT" \
  --apply --reason "restore into $PROJECT" --ack-external-replay

echo "starting the server"
"${COMPOSE[@]}" up -d --wait server

python3 -c 'import json,sys; json.dump({"restoredProject": sys.argv[1], "searchRebuilt": "ensure-meili-key"}, sys.stdout)' "$PROJECT"
printf '\n'
