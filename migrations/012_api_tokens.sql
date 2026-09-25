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
