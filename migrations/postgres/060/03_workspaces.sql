-- FVOCI PostgreSQL baseline fvoci-postgres-060, step 03: workspaces.
-- groups, invitations, API tokens, workspace SSO configuration, holidays, the typed
-- instance settings store and the seat-quota aggregate. users/workspaces/memberships are
-- step 02 objects.

-- Instance seat admission needs one aggregate over tenant memberships without
-- exposing membership rows or widening their RLS policy.
CREATE FUNCTION fvoci.app_quota_billable_users(p_user_id uuid)
RETURNS integer
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
BEGIN
  IF current_setting('app.system_ctx', true) IS DISTINCT FROM 'on' THEN
    RAISE EXCEPTION 'quota billable user aggregate requires system context';
  END IF;
  RETURN (
    SELECT count(*)::integer
    FROM fvoci.users u
    WHERE (p_user_id IS NULL OR u.id = p_user_id)
      AND u.anonymized_at IS NULL
      AND (
        u.is_instance_admin
        OR EXISTS (
          SELECT 1
          FROM fvoci.memberships m
          JOIN fvoci.workspaces w ON w.id = m.workspace_id
          WHERE m.user_id = u.id
            AND m.role <> 'guest'
            AND w.kind = 'team'
            AND w.deleted_at IS NULL
        )
        OR EXISTS (
          SELECT 1
          FROM fvoci.memberships m
          JOIN fvoci.workspaces w ON w.id = u.personal_workspace_id
          WHERE m.workspace_id = w.id
            AND m.user_id = u.id
            AND m.role = 'owner'
            AND w.kind = 'personal'
            AND w.deleted_at IS NULL
        )
      )
  );
END;
$$;

CREATE TABLE fvoci.groups (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    name text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT groups_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT groups_name_check CHECK (
        char_length(btrim(name)) >= 1 AND char_length(name) <= 100
    )
);

ALTER TABLE fvoci.groups ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.groups FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.groups
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.group_members (
    workspace_id uuid NOT NULL,
    group_id uuid NOT NULL,
    user_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, group_id, user_id),
    CONSTRAINT group_members_workspace_group_fk
        FOREIGN KEY (workspace_id, group_id)
        REFERENCES fvoci.groups (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT group_members_workspace_user_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id)
        ON DELETE CASCADE
);

CREATE INDEX group_members_workspace_id_user_id_idx
    ON fvoci.group_members (workspace_id, user_id);

ALTER TABLE fvoci.group_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.group_members FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.group_members
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.invitations (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    email text NOT NULL,
    role text NOT NULL,
    token_hash text NOT NULL,
    invited_by uuid NOT NULL REFERENCES fvoci.users (id),
    expires_at timestamptz NOT NULL,
    accepted_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT invitations_token_hash_unique UNIQUE (token_hash),
    CONSTRAINT invitations_email_canonical_check CHECK (
        email ~ '^[!-~]+$' AND email = lower(email COLLATE "C")
    ),
    CONSTRAINT invitations_role_check CHECK (role IN ('owner', 'admin', 'member', 'guest'))
);

CREATE INDEX invitations_workspace_id_invited_by_idx
    ON fvoci.invitations (workspace_id, invited_by);

ALTER TABLE fvoci.invitations ENABLE ROW LEVEL SECURITY;

CREATE POLICY invitations_select ON fvoci.invitations
    AS PERMISSIVE FOR SELECT TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR token_hash = (SELECT public.app_invitation_token_hash())
    );

CREATE POLICY invitations_tenant_insert ON fvoci.invitations
    AS PERMISSIVE FOR INSERT TO public
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE POLICY invitations_tenant_update ON fvoci.invitations
    AS PERMISSIVE FOR UPDATE TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE POLICY invitations_tenant_delete ON fvoci.invitations
    AS PERMISSIVE FOR DELETE TO public
    USING (workspace_id = (SELECT public.app_tenant_id()));

-- Personal API tokens: scopes are a closed set; a token belongs to a membership
-- and disappears with it.
CREATE TABLE fvoci.api_tokens (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    user_id uuid,
    token_hash text NOT NULL,
    name text NOT NULL,
    scopes text[] NOT NULL,
    expires_at timestamptz,
    last_used_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT api_tokens_token_hash_unique UNIQUE (token_hash),
    CONSTRAINT api_tokens_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT api_tokens_name_len_check CHECK (char_length(name) BETWEEN 1 AND 100),
    CONSTRAINT api_tokens_scopes_check CHECK (
        scopes <@ ARRAY[
            'documents.read',
            'documents.write',
            'tasks.read',
            'tasks.write',
            'projects.read',
            'projects.manage',
            'share.manage',
            'workspace.manage'
        ]::text[]
        AND cardinality(scopes) > 0
    ),
    CONSTRAINT api_tokens_workspace_user_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id)
        ON DELETE CASCADE
);

CREATE INDEX api_tokens_workspace_id_user_id_idx
    ON fvoci.api_tokens (workspace_id, user_id);

ALTER TABLE fvoci.api_tokens ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON fvoci.api_tokens
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    )
    WITH CHECK (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

-- client_secret is sealed (AAD `workspace-oidc:<workspace_id>`).
CREATE TABLE fvoci.workspace_oidc (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    issuer text NOT NULL,
    client_id text NOT NULL,
    client_secret text NOT NULL,
    label text NOT NULL DEFAULT 'SSO',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT workspace_oidc_workspace_id_unique UNIQUE (workspace_id),
    CONSTRAINT workspace_oidc_secret_sealed_check CHECK (client_secret LIKE 'enc:v2:%')
);

ALTER TABLE fvoci.workspace_oidc ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.workspace_oidc FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.workspace_oidc
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    )
    WITH CHECK (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

-- Workspace SSO entry by slug before sign-in (system transaction): only the id
-- of a live workspace that has a configuration, nothing else.
CREATE FUNCTION fvoci.app_workspace_sso_id_by_slug(p_slug text)
RETURNS uuid
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT w.id
    FROM fvoci.workspaces AS w
    JOIN fvoci.workspace_oidc AS o ON o.workspace_id = w.id
    WHERE w.slug = p_slug AND w.deleted_at IS NULL
$$;

CREATE TABLE fvoci.workspace_holidays (
    workspace_id uuid NOT NULL
        REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    date date NOT NULL,
    CONSTRAINT workspace_holidays_workspace_id_date_pk PRIMARY KEY (workspace_id, date)
);

ALTER TABLE fvoci.workspace_holidays ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.workspace_holidays FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.workspace_holidays
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

-- Typed instance settings store with a monotonic change token (the source's
-- in-memory reload counter; this server reads the rows per request).
CREATE TABLE fvoci.instance_settings (
    key text PRIMARY KEY,
    value jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT instance_settings_key_check CHECK (key ~ '^[A-Za-z][A-Za-z0-9.]{0,63}$')
);

CREATE TABLE fvoci.instance_settings_meta (
    id smallint PRIMARY KEY DEFAULT 1,
    revision bigint NOT NULL DEFAULT 0,
    CONSTRAINT instance_settings_meta_singleton_check CHECK (id = 1)
);
-- Seed: the singleton revision row exists on every install.
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

REVOKE ALL ON FUNCTION fvoci.app_workspace_sso_id_by_slug(text) FROM PUBLIC;
