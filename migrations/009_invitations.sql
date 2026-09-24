CREATE FUNCTION public.app_invitation_token_hash()
RETURNS text
LANGUAGE sql
STABLE
SET search_path = pg_catalog
AS $$ SELECT NULLIF(pg_catalog.current_setting('app.invitation_token_hash', true), '') $$;

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
