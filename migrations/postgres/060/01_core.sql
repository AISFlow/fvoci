-- FVOCI PostgreSQL new-install baseline, lineage fvoci-postgres-060, step 01: schema, ledger, context functions.
--
-- The baseline is derived from the FINAL catalog of the retired development lineage
-- (migrations 001..056 of the 0.5/0.6 development tree). It installs into an empty
-- database only. A database carrying the retired lineage (a fvoci.schema_migrations
-- without a lineage column) is refused by the runner and never rewritten; data moves
-- between installs with a current-format native archive.
--
-- Every step is applied in its own transaction under the migration advisory lock.
-- The runner records (version, lineage, sql_sha256) in the same transaction as the
-- step's DDL and revokes PUBLIC EXECUTE from every SECURITY DEFINER function the
-- migration owner created. scripts/grant-app-role.sql grants the restricted app role
-- afterwards; the grant script is the only privilege source for the app role.

CREATE SCHEMA IF NOT EXISTS fvoci;

-- Step receipts of this lineage only. version is the 1-based position of the compiled
-- step; sql_sha256 is the digest of the exact compiled SQL text of that step.
CREATE TABLE fvoci.schema_migrations (
    version integer PRIMARY KEY,
    lineage text NOT NULL,
    sql_sha256 text NOT NULL,
    applied_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT schema_migrations_version_check CHECK (version > 0),
    CONSTRAINT schema_migrations_lineage_check CHECK (lineage = 'fvoci-postgres-060'),
    CONSTRAINT schema_migrations_sql_sha256_check CHECK (sql_sha256 ~ '^[0-9a-f]{64}$')
);

-- Transaction-local request context read by every RLS policy and definer.
-- app.tenant_id: the workspace of the request; app.self_user_id: the authenticated
-- user; app.system_ctx = 'on': a named system transaction (outbox relay, sign-in,
-- maintenance); app.invitation_token_hash: an invitation lookup before membership.
CREATE FUNCTION public.app_tenant_id()
RETURNS uuid
LANGUAGE sql
STABLE
SET search_path = pg_catalog
AS $$ SELECT NULLIF(pg_catalog.current_setting('app.tenant_id', true), '')::uuid $$;

CREATE FUNCTION public.app_system_ctx_on()
RETURNS boolean
LANGUAGE sql
STABLE
SET search_path = pg_catalog
AS $$ SELECT NULLIF(pg_catalog.current_setting('app.system_ctx', true), '') = 'on' $$;

CREATE FUNCTION public.app_self_user_id()
RETURNS uuid
LANGUAGE sql
STABLE
SET search_path = pg_catalog
AS $$ SELECT NULLIF(pg_catalog.current_setting('app.self_user_id', true), '')::uuid $$;

CREATE FUNCTION public.app_invitation_token_hash()
RETURNS text
LANGUAGE sql
STABLE
SET search_path = pg_catalog
AS $$ SELECT NULLIF(pg_catalog.current_setting('app.invitation_token_hash', true), '') $$;
