-- Instance administration: legal documents and consents, the typed instance
-- settings store, and the narrow definers the admin console and the 428
-- consent gate need. Source: packages/db pg/schema identity.ts
-- (legal_documents, user_consents) and ops.ts (instance_settings).

CREATE TABLE fvoci.legal_documents (
    id uuid PRIMARY KEY,
    kind text NOT NULL,
    version integer NOT NULL,
    title text NOT NULL,
    body_markdown text NOT NULL,
    -- Rendered once at publish (markdown -> editor schema -> sanitized HTML,
    -- source tiptapDocToSafeHtml); rows are immutable, so reads never render.
    body_html text NOT NULL,
    required boolean NOT NULL,
    effective_at timestamptz NOT NULL,
    published_at timestamptz NOT NULL,
    created_by uuid NOT NULL REFERENCES fvoci.users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT legal_documents_kind_version_unique UNIQUE (kind, version),
    CONSTRAINT legal_documents_kind_shape_check CHECK (kind ~ '^[a-z0-9-]{1,50}$'),
    CONSTRAINT legal_documents_version_check CHECK (version >= 1)
);

-- Rows are append-only: the app role gets SELECT and INSERT only
-- (scripts/grant-app-role.sql). Anyone may read published documents
-- (GET /legal/:kind is public); only an instance-admin transaction that has
-- switched to the system context may publish.
ALTER TABLE fvoci.legal_documents ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.legal_documents FORCE ROW LEVEL SECURITY;

CREATE POLICY legal_documents_select ON fvoci.legal_documents
    FOR SELECT USING (true);

CREATE POLICY legal_documents_insert ON fvoci.legal_documents
    FOR INSERT WITH CHECK ((SELECT public.app_system_ctx_on()));

CREATE TABLE fvoci.user_consents (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES fvoci.users (id),
    kind text NOT NULL,
    version integer NOT NULL,
    consented_at timestamptz NOT NULL,
    ip inet,
    channel text NOT NULL,
    CONSTRAINT user_consents_user_id_kind_version_unique UNIQUE (user_id, kind, version),
    CONSTRAINT user_consents_channel_check CHECK (channel IN ('signup', 'gate'))
);

-- A user reads and records their own consents; a workspace admin reads the
-- consents of the tenant's members (GET /workspaces/:id/consents, after the
-- in-transaction admin check). Consents are evidence: append-only.
ALTER TABLE fvoci.user_consents ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.user_consents FORCE ROW LEVEL SECURITY;

CREATE POLICY user_consents_select ON fvoci.user_consents
    FOR SELECT USING (
        user_id = (SELECT public.app_self_user_id())
        OR (SELECT public.app_system_ctx_on())
        OR EXISTS (
            SELECT 1 FROM fvoci.memberships m
            WHERE m.user_id = user_consents.user_id
              AND m.workspace_id = (SELECT public.app_tenant_id())
        )
    );

CREATE POLICY user_consents_insert ON fvoci.user_consents
    FOR INSERT WITH CHECK (
        user_id = (SELECT public.app_self_user_id())
        OR (SELECT public.app_system_ctx_on())
    );

CREATE TABLE fvoci.instance_settings (
    key text PRIMARY KEY,
    value jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT instance_settings_key_check CHECK (key ~ '^[A-Za-z][A-Za-z0-9.]{0,63}$')
);

-- Monotonic change token for the settings document (the source's in-memory
-- reload counter; this server reads the rows per request instead of caching).
CREATE TABLE fvoci.instance_settings_meta (
    id smallint PRIMARY KEY DEFAULT 1,
    revision bigint NOT NULL DEFAULT 0,
    CONSTRAINT instance_settings_meta_singleton_check CHECK (id = 1)
);
INSERT INTO fvoci.instance_settings_meta (id, revision) VALUES (1, 0);

-- Public settings are served to anonymous clients, so reads are open; writes
-- happen only inside an instance-admin transaction in the system context.
ALTER TABLE fvoci.instance_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.instance_settings FORCE ROW LEVEL SECURITY;
ALTER TABLE fvoci.instance_settings_meta ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.instance_settings_meta FORCE ROW LEVEL SECURITY;

CREATE POLICY instance_settings_select ON fvoci.instance_settings
    FOR SELECT USING (true);
CREATE POLICY instance_settings_insert ON fvoci.instance_settings
    FOR INSERT WITH CHECK ((SELECT public.app_system_ctx_on()));
CREATE POLICY instance_settings_update ON fvoci.instance_settings
    FOR UPDATE USING ((SELECT public.app_system_ctx_on()))
    WITH CHECK ((SELECT public.app_system_ctx_on()));
CREATE POLICY instance_settings_delete ON fvoci.instance_settings
    FOR DELETE USING ((SELECT public.app_system_ctx_on()));

CREATE POLICY instance_settings_meta_select ON fvoci.instance_settings_meta
    FOR SELECT USING (true);
CREATE POLICY instance_settings_meta_update ON fvoci.instance_settings_meta
    FOR UPDATE USING ((SELECT public.app_system_ctx_on()))
    WITH CHECK ((SELECT public.app_system_ctx_on()));

-- The 428 consent gate: does the live session behind this token hash belong
-- to a live user who has not consented to the latest version of every kind
-- whose latest version is required? Returns only that boolean.
CREATE FUNCTION fvoci.app_session_consent_pending(p_hash text)
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM fvoci.sessions s
        INNER JOIN fvoci.users u ON u.id = s.user_id
        INNER JOIN (
            SELECT DISTINCT ON (d.kind) d.kind, d.version, d.required
            FROM fvoci.legal_documents d
            ORDER BY d.kind, d.version DESC
        ) latest ON latest.required
        WHERE s.token_hash = p_hash
          AND s.revoked_at IS NULL
          AND s.expires_at > now()
          AND u.deleted_at IS NULL
          AND u.suspended_at IS NULL
          AND NOT EXISTS (
              SELECT 1 FROM fvoci.user_consents c
              WHERE c.user_id = u.id
                AND c.kind = latest.kind
                AND c.version = latest.version
          )
    )
$$;

-- users.is_instance_admin / suspended_at are not app-role writable columns.
-- These definers change one flag of one live user, only when the calling
-- transaction's self user (app.self_user_id) is a live instance admin, and
-- refuse any change that would leave the instance without a live admin.
-- The route still rechecks authorization and the last-admin rule under the
-- instance-admin advisory lock; this is the database floor under it.
CREATE FUNCTION fvoci.app_admin_set_instance_admin(p_target uuid, p_value boolean)
RETURNS integer
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    changed integer;
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM fvoci.users a
        WHERE a.id = public.app_self_user_id()
          AND a.is_instance_admin
          AND a.deleted_at IS NULL
          AND a.suspended_at IS NULL
    ) THEN
        RAISE EXCEPTION 'instance admin required' USING ERRCODE = '42501';
    END IF;
    UPDATE fvoci.users
    SET is_instance_admin = p_value, updated_at = now()
    WHERE id = p_target AND deleted_at IS NULL AND is_instance_admin IS DISTINCT FROM p_value;
    GET DIAGNOSTICS changed = ROW_COUNT;
    IF NOT EXISTS (
        SELECT 1 FROM fvoci.users
        WHERE is_instance_admin AND deleted_at IS NULL AND suspended_at IS NULL
    ) THEN
        RAISE EXCEPTION 'last instance admin' USING ERRCODE = '23514';
    END IF;
    RETURN changed;
END
$$;

CREATE FUNCTION fvoci.app_admin_set_suspended(p_target uuid, p_suspended boolean)
RETURNS integer
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    changed integer;
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM fvoci.users a
        WHERE a.id = public.app_self_user_id()
          AND a.is_instance_admin
          AND a.deleted_at IS NULL
          AND a.suspended_at IS NULL
    ) THEN
        RAISE EXCEPTION 'instance admin required' USING ERRCODE = '42501';
    END IF;
    IF p_suspended THEN
        UPDATE fvoci.users
        SET suspended_at = now(), updated_at = now()
        WHERE id = p_target AND deleted_at IS NULL AND suspended_at IS NULL;
    ELSE
        UPDATE fvoci.users
        SET suspended_at = NULL, updated_at = now()
        WHERE id = p_target AND deleted_at IS NULL AND suspended_at IS NOT NULL;
    END IF;
    GET DIAGNOSTICS changed = ROW_COUNT;
    IF NOT EXISTS (
        SELECT 1 FROM fvoci.users
        WHERE is_instance_admin AND deleted_at IS NULL AND suspended_at IS NULL
    ) THEN
        RAISE EXCEPTION 'last instance admin' USING ERRCODE = '23514';
    END IF;
    RETURN changed;
END
$$;
