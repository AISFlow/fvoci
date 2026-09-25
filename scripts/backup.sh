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
# Pepper keys, DB passwords, and the Meili master key stay in the operator
# env file — they are not copied into the archive (beyond whatever the database
# dump already contains).
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

STORAGE_VOL="$(docker inspect -f '{{range .Mounts}}{{if eq .Destination "/data/storage"}}{{.Name}}{{end}}{{end}}' "$SERVER_CID")"
if [[ -z "$STORAGE_VOL" ]]; then
  echo "server container has no /data/storage volume" >&2
  exit 1
fi
docker volume inspect "$STORAGE_VOL" >/dev/null

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
STORED_KEYS="$("${COMPOSE[@]}" exec -T postgres sh -c \
  'psql -X -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" -tAc "SELECT storage_key FROM fvoci.attachments WHERE status='\''stored'\''"')"
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
DUMP_SIZE="$(stat -c '%s' "$DUMP")"
DUMP_SHA="$(sha256sum "$DUMP" | awk '{print $1}')"
TAR_SIZE="$(stat -c '%s' "$STAGING/storage.tar")"
TAR_SHA="$(sha256sum "$STAGING/storage.tar" | awk '{print $1}')"
PG_VERSION="$("${COMPOSE[@]}" exec -T postgres sh -c 'psql -X -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" -tAc "SHOW server_version_num"')"
PG_VERSION="$(printf '%s' "$PG_VERSION" | tr -d '[:space:]')"

PEPPER_FP="$(pepper_fingerprint "$(read_env PASSWORD_PEPPER_KEYS)" "$(read_env PASSWORD_PEPPER_ACTIVE_KEY_ID)")"
PEPPER_FP="$PEPPER_FP" CREATED_AT="$CREATED_AT" SOURCE_PROJECT="$PROJECT" \
  DUMP_SIZE="$DUMP_SIZE" DUMP_SHA="$DUMP_SHA" \
  TAR_SIZE="$TAR_SIZE" TAR_SHA="$TAR_SHA" \
  PG_VERSION="$PG_VERSION" \
  python3 - "$STAGING/manifest.json" <<'PY'
import json, os, sys
manifest = {
    "formatVersion": 1,
    "createdAt": os.environ["CREATED_AT"],
    "sourceProject": os.environ["SOURCE_PROJECT"],
    "schema": "fvoci",
    "postgres": {"serverVersionNum": int(os.environ["PG_VERSION"])},
    "passwordPepper": {
        "fingerprint": os.environ["PEPPER_FP"],
        "note": "SHA-256 of the canonical keyring and active id; keys are not stored. Restore refuses a different keyring because existing password hashes could not be verified.",
    },
    "search": {
        "included": False,
        "reason": "Meilisearch is derived. Restore runs fvoci-migrate --ensure-meili-key (scoped key and index settings). Product search-rebuild is not in this slice; extract_text lives in PostgreSQL.",
    },
    "database": {
        "path": "database.dump",
        "sizeBytes": int(os.environ["DUMP_SIZE"]),
        "sha256": os.environ["DUMP_SHA"],
    },
    "storage": {
        "path": "storage.tar",
        "sizeBytes": int(os.environ["TAR_SIZE"]),
        "sha256": os.environ["TAR_SHA"],
    },
}
with open(sys.argv[1], "w", encoding="utf-8") as fh:
    json.dump(manifest, fh, indent=2)
    fh.write("\n")
PY
chmod 600 "$STAGING/manifest.json"

mv --no-target-directory --no-clobber "$STAGING" "$OUTPUT"
chmod 700 "$OUTPUT"

if (( LEAVE_STOPPED == 0 )); then
  "${COMPOSE[@]}" up -d --wait server
  SERVER_STOPPED=0
fi

python3 -c 'import json,sys; json.dump({"backup": sys.argv[1], "objectsChecked": True}, sys.stdout)' "$OUTPUT"
printf '\n'
