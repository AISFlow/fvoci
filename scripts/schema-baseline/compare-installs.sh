#!/usr/bin/env bash
# One-time semantic catalog comparison driver (R3): installs the retired
# development lineage with a QUALIFIED pre-transition fvoci-migrate into
# database A and the fvoci-postgres-060 baseline with this tree's fvoci-migrate
# into database B on the same PostgreSQL server, grants the same app role on
# both, dumps both catalogs with tests/schema_baseline_integration.rs
# (postgres_catalog_dump) and compares them with compare-catalogs.py.
#
#   scripts/schema-baseline/compare-installs.sh \
#       --server-url postgres://owner:***@127.0.0.1:PORT/postgres \
#       --old-migrate /path/to/qualified/pre-transition/fvoci-migrate \
#       --new-migrate /path/to/this-tree/fvoci-migrate \
#       --cargo-target /path/to/this-tree/target \
#       --out DIR
#
# The server is a throwaway (root-allocated) PostgreSQL 18; the script creates
# two databases and one app role named after the run id, and drops them at the
# end unless --keep is given. It never touches another database. Direct SQL
# application is deliberately not offered: the old installer must be the
# qualified binary (root locates it); this script refuses to improvise one.
set -euo pipefail
SERVER_URL=""; OLD_MIGRATE=""; NEW_MIGRATE=""; CARGO_TARGET=""; OUT=""; KEEP=0
while (($#)); do
  case "$1" in
    --server-url) SERVER_URL="${2:?}"; shift 2 ;;
    --old-migrate) OLD_MIGRATE="${2:?}"; shift 2 ;;
    --new-migrate) NEW_MIGRATE="${2:?}"; shift 2 ;;
    --cargo-target) CARGO_TARGET="${2:?}"; shift 2 ;;
    --out) OUT="${2:?}"; shift 2 ;;
    --keep) KEEP=1; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[[ -n "$SERVER_URL" && -n "$OLD_MIGRATE" && -n "$NEW_MIGRATE" && -n "$CARGO_TARGET" && -n "$OUT" ]] || { sed -n '2,20p' "$0" >&2; exit 2; }
[[ -x "$OLD_MIGRATE" && -x "$NEW_MIGRATE" ]] || { echo "migrate binaries must be executable" >&2; exit 2; }
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
mkdir -p "$OUT"
RUN_ID="$(python3 -c 'import secrets; print(secrets.token_hex(4))')"
DB_A="fvoci_cmp_old_${RUN_ID}"; DB_B="fvoci_cmp_new_${RUN_ID}"; ROLE="fvoci_app_cmp_${RUN_ID}"
PASS="$(python3 -c 'import secrets; print(secrets.token_hex(16))')"
PSQL="${PSQL:-}"
[[ -n "$PSQL" ]] || { echo "set PSQL to a psql command that reaches the throwaway server (for example: docker exec -i <ctr> psql -U <owner>)" >&2; exit 2; }
sql() { local db="$1"; shift; $PSQL -X -v ON_ERROR_STOP=1 -d "$db" -c "$*"; }
db_url() { python3 - "$SERVER_URL" "$1" <<'PY'
import sys, urllib.parse
u = urllib.parse.urlsplit(sys.argv[1]); print(urllib.parse.urlunsplit((u.scheme, u.netloc, '/' + sys.argv[2], u.query, u.fragment)))
PY
}
record() { printf '%s %s\n' "$(date -Is)" "$*" | tee -a "$OUT/steps.log"; }
cleanup() {
  if (( KEEP )); then record "keeping $DB_A $DB_B $ROLE"; return; fi
  for db in "$DB_A" "$DB_B"; do
    sql postgres "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '$db'" >/dev/null || true
    sql postgres "DROP DATABASE IF EXISTS \"$db\"" >/dev/null || true
  done
  sql postgres "DROP ROLE IF EXISTS \"$ROLE\"" >/dev/null || true
  record "dropped $DB_A $DB_B $ROLE"
}
trap cleanup EXIT
record "run $RUN_ID old-migrate=$(sha256sum "$OLD_MIGRATE" | cut -c1-16) new-migrate=$(sha256sum "$NEW_MIGRATE" | cut -c1-16) tree=$(git -C "$ROOT" rev-parse HEAD)"
sql postgres "CREATE ROLE \"$ROLE\" LOGIN PASSWORD '$PASS' NOSUPERUSER NOBYPASSRLS" >/dev/null
for db in "$DB_A" "$DB_B"; do sql postgres "CREATE DATABASE \"$db\"" >/dev/null; done
record "A: retired lineage via qualified old migrate"
DATABASE_URL="$(db_url "$DB_A")" "$OLD_MIGRATE" > "$OUT/a-migrate.log" 2>&1
DATABASE_URL="$(db_url "$DB_A")" "$OLD_MIGRATE" --grant-app-role "$ROLE" >> "$OUT/a-migrate.log" 2>&1
record "B: fvoci-postgres-060 via this tree's migrate"
DATABASE_URL="$(db_url "$DB_B")" "$NEW_MIGRATE" > "$OUT/b-migrate.log" 2>&1
DATABASE_URL="$(db_url "$DB_B")" "$NEW_MIGRATE" --grant-app-role "$ROLE" >> "$OUT/b-migrate.log" 2>&1
record "dump both catalogs"
for side in a b; do
  db="$DB_A"; [[ "$side" == b ]] && db="$DB_B"
  ( cd "$ROOT" && FVOCI_SCHEMA_CATALOG_DATABASE_URL="$(db_url "$db")" FVOCI_SCHEMA_CATALOG_APP_ROLE="$ROLE" \
      FVOCI_SCHEMA_CATALOG_OUT="$OUT/$side.json" CARGO_TARGET_DIR="$CARGO_TARGET" \
      cargo test --locked --offline --features db-tests --test schema_baseline_integration -- --exact postgres_catalog_dump ) > "$OUT/$side-dump.log" 2>&1
done
record "compare"
set +e
python3 "$ROOT/scripts/schema-baseline/compare-catalogs.py" "$OUT/a.json" "$OUT/b.json" --report "$OUT/report.md"
RC=$?
set -e
record "compare exit=$RC (0 = semantic PASS, only the ledger differs)"
exit "$RC"
