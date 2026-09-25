#!/bin/sh
# One-shot database bootstrap for compose: create the non-superuser app role, migrate,
# then apply grant-app-role.sql through fvoci-migrate. Requires owner DATABASE_URL.
set -eu

: "${DATABASE_URL:?DATABASE_URL is required}"
: "${FVOCI_APP_ROLE:?FVOCI_APP_ROLE is required}"
: "${FVOCI_APP_PASSWORD:?FVOCI_APP_PASSWORD is required}"

# Role name and password are passed as psql variables and quoted by format()
# (%I / %L), never interpolated into SQL text by the shell.
psql -X -v ON_ERROR_STOP=1 \
  -v app_role="$FVOCI_APP_ROLE" -v app_password="$FVOCI_APP_PASSWORD" \
  "$DATABASE_URL" <<'SQL'
SELECT format('CREATE ROLE %I LOGIN PASSWORD %L NOSUPERUSER NOBYPASSRLS', :'app_role', :'app_password')
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = :'app_role')
\gexec
SQL

/opt/fvoci/bin/fvoci-migrate
/opt/fvoci/bin/fvoci-migrate --grant-app-role "$FVOCI_APP_ROLE"

if [ -n "${FVOCI_MEILI_URL:-}" ]; then
  : "${MEILI_MASTER_KEY:?MEILI_MASTER_KEY is required when FVOCI_MEILI_URL is set}"
  KEY_FILE="${FVOCI_MEILI_KEY_FILE:-/run/fvoci/meili/api_key}"
  /opt/fvoci/bin/fvoci-migrate --ensure-meili-key "$KEY_FILE"
fi
