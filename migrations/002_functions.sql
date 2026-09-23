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

CREATE FUNCTION fvoci.app_user_password_hash(p_id uuid)
RETURNS text
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT password_hash FROM fvoci.users
    WHERE id = p_id AND deleted_at IS NULL
$$;

CREATE FUNCTION fvoci.app_session_by_token_hash(p_hash text)
RETURNS SETOF fvoci.sessions
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT * FROM fvoci.sessions
    WHERE token_hash = p_hash
      AND revoked_at IS NULL
      AND expires_at > now()
$$;

CREATE FUNCTION fvoci.app_user_rehash_password_hash(p_id uuid, p_hash text, p_expected text)
RETURNS integer
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    WITH updated AS (
        UPDATE fvoci.users
        SET password_hash = p_hash, updated_at = now()
        WHERE id = p_id AND deleted_at IS NULL AND password_hash = p_expected
        RETURNING 1
    )
    SELECT count(*)::integer FROM updated
$$;

ALTER TABLE fvoci.workspaces ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.events ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.events FORCE ROW LEVEL SECURITY;
ALTER TABLE fvoci.audit_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.audit_log FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON fvoci.workspaces
    AS PERMISSIVE FOR ALL TO public
    USING (
        id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

CREATE POLICY tenant_isolation ON fvoci.memberships
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()));

CREATE POLICY events_insert ON fvoci.events
    FOR INSERT WITH CHECK (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

CREATE POLICY events_select ON fvoci.events
    FOR SELECT USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

CREATE POLICY audit_log_select ON fvoci.audit_log
    FOR SELECT USING ((SELECT public.app_system_ctx_on()));

CREATE POLICY audit_log_insert ON fvoci.audit_log
    FOR INSERT WITH CHECK (true);
