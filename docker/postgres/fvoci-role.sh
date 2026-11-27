#!/bin/sh
# Run by the compose service `roles` at every `up`, before `migrate`: as PostgreSQL's
# superuser (PGUSER, `postgres`, the owner of the tables), over the network, in the
# database PGDATABASE (`fvoci`). Makes the role the fvoci server connects as, or makes
# it what it must be, with the password of the secret: a role that was not made, or a
# password that changed, is set right at the next `up`.
#
# The role `fvoci` logs in and is no superuser, has no BYPASSRLS, and makes no databases
# or roles. It owns nothing: the schema `public` and its tables are the superuser's, who
# brings them up to date (the service `migrate`). It may only use the tables (SELECT,
# INSERT, UPDATE, DELETE; only SELECT of the record of the migrations) and the
# sequences, those there now and, by default
# privileges, those the migrations make later. So the row-level security policies that
# keep organizations apart hold it, and it cannot turn them off: only an owner may alter
# a table. It may connect to no other database.
#
#   POSTGRES_PASSWORD_FILE  the superuser's password (a Docker secret)
#   FVOCI_DB_PASSWORD_FILE  the role's password (a Docker secret)
set -eu

for file in "${POSTGRES_PASSWORD_FILE:?the file of the superuser's password}" \
    "${FVOCI_DB_PASSWORD_FILE:?the file of the password of the role fvoci connects as}"; do
    [ -s "$file" ] || { echo "fvoci-role: $file is empty or cannot be read" >&2; exit 1; }
done
PGPASSWORD=$(tr -d '\r' <"$POSTGRES_PASSWORD_FILE")
export PGPASSWORD

# The role's password is read by psql itself (\set with a backquote), so it is on no
# command line, and quoted by psql (:'password').
psql --no-psqlrc --quiet --set ON_ERROR_STOP=1 --set role=fvoci \
    --set file="$FVOCI_DB_PASSWORD_FILE" <<'SQL'
SELECT NOT EXISTS (SELECT FROM pg_roles WHERE rolname = :'role') AS missing \gset
\if :missing
CREATE ROLE :"role";
\endif
\set password `tr -d '\r' < :'file'`
SET log_statement = 'none';
SET log_min_duration_statement = -1;
SET log_min_error_statement = panic;
ALTER ROLE :"role" LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOREPLICATION
    PASSWORD :'password';
RESET log_min_error_statement;
RESET log_min_duration_statement;
RESET log_statement;
\unset password
REVOKE ALL ON DATABASE :"DBNAME" FROM PUBLIC;
GRANT CONNECT ON DATABASE :"DBNAME" TO :"role";
REVOKE CONNECT, TEMPORARY ON DATABASE postgres, template1 FROM PUBLIC;
REVOKE ALL ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO :"role";
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO :"role";
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO :"role";
ALTER DEFAULT PRIVILEGES FOR ROLE :"USER" IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO :"role";
ALTER DEFAULT PRIVILEGES FOR ROLE :"USER" IN SCHEMA public
    GRANT USAGE, SELECT ON SEQUENCES TO :"role";
-- The record of the migrations is read, never written, by the server: a server broken
-- into does not make the schema read as another build's. (Made by the first `migrate`,
-- after the first `roles`: taken back from the next `up` on.)
SELECT to_regclass('public._sqlx_migrations') IS NOT NULL AS recorded \gset
\if :recorded
REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON public._sqlx_migrations FROM :"role";
\endif
SQL
echo "fvoci-role: the role fvoci, its password and its grants are set"
