#!/bin/sh
# One-shot database bootstrap for compose: create the non-superuser app role, migrate,
# then apply grant-app-role.sql through fvoci-migrate. Requires owner DATABASE_URL.
set -eu

: "${DATABASE_URL:?DATABASE_URL is required}"
: "${FVOCI_APP_ROLE:?FVOCI_APP_ROLE is required}"
: "${FVOCI_APP_PASSWORD:?FVOCI_APP_PASSWORD is required}"

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 <<SQL
DO \$\$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '${FVOCI_APP_ROLE}') THEN
    EXECUTE format(
      'CREATE ROLE %I LOGIN PASSWORD %L NOSUPERUSER NOBYPASSRLS',
      '${FVOCI_APP_ROLE}',
      '${FVOCI_APP_PASSWORD}'
    );
  END IF;
END
\$\$;
SQL

/opt/fvoci/bin/fvoci-migrate
/opt/fvoci/bin/fvoci-migrate --grant-app-role "$FVOCI_APP_ROLE"
