-- Stars (per-user favourites) and public share links.
-- 023 is reserved for the import/export branch; the coordinator renumbers at
-- integration if needed.

CREATE TABLE fvoci.stars (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    user_id uuid NOT NULL,
    document_id uuid,
    task_id uuid,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT stars_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT stars_parent_xor_check CHECK ((document_id IS NULL) <> (task_id IS NULL)),
    CONSTRAINT stars_workspace_user_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id)
        ON DELETE CASCADE,
    CONSTRAINT stars_workspace_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT stars_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE
);

CREATE UNIQUE INDEX stars_user_document_unique
    ON fvoci.stars (workspace_id, user_id, document_id);
CREATE UNIQUE INDEX stars_user_task_unique
    ON fvoci.stars (workspace_id, user_id, task_id);
CREATE INDEX stars_workspace_id_document_id_idx ON fvoci.stars (workspace_id, document_id);
CREATE INDEX stars_workspace_id_task_id_idx ON fvoci.stars (workspace_id, task_id);

ALTER TABLE fvoci.stars ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.stars FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.stars
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

-- token_hash is SHA-256 hex of a 256-bit random token; the raw token is never
-- stored. The app role cannot SELECT token_hash (column grant in
-- grant-app-role.sql); public resolution goes through the definer function
-- below, which also requires the system context.
CREATE TABLE fvoci.share_links (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    user_id uuid NOT NULL,
    token_hash text NOT NULL,
    document_id uuid,
    project_id uuid,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT share_links_token_hash_unique UNIQUE (token_hash),
    CONSTRAINT share_links_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT share_links_target_xor_check CHECK ((document_id IS NULL) <> (project_id IS NULL)),
    CONSTRAINT share_links_token_hash_format_check CHECK (token_hash ~ '^[0-9a-f]{64}$'),
    CONSTRAINT share_links_workspace_user_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id)
        ON DELETE CASCADE,
    CONSTRAINT share_links_workspace_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT share_links_workspace_project_fk
        FOREIGN KEY (workspace_id, project_id)
        REFERENCES fvoci.projects (workspace_id, id)
        ON DELETE CASCADE
);

CREATE INDEX share_links_workspace_id_document_id_idx
    ON fvoci.share_links (workspace_id, document_id);
CREATE INDEX share_links_workspace_id_project_id_idx
    ON fvoci.share_links (workspace_id, project_id);
CREATE INDEX share_links_workspace_id_user_id_idx
    ON fvoci.share_links (workspace_id, user_id);

ALTER TABLE fvoci.share_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.share_links FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.share_links
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    )
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

-- Public token resolution: exact hash match, unexpired only. Returns the
-- stored hash so the caller can re-compare in constant time; no other secret
-- leaves the table. The caller must enable the system context for the
-- transaction; the predicate is explicit so it holds even when the function
-- owner bypasses RLS.
CREATE FUNCTION fvoci.app_share_link_by_token_hash(p_hash text)
RETURNS TABLE (
    id uuid,
    workspace_id uuid,
    token_hash text,
    document_id uuid,
    project_id uuid,
    expires_at timestamptz
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT s.id, s.workspace_id, s.token_hash, s.document_id, s.project_id, s.expires_at
    FROM fvoci.share_links s
    WHERE s.token_hash = p_hash
      AND s.expires_at > now()
      AND (SELECT public.app_system_ctx_on())
$$;
