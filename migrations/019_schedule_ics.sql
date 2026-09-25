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

CREATE TABLE fvoci.ics_tokens (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    user_id uuid NOT NULL,
    token_hash text NOT NULL,
    expires_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT ics_tokens_token_hash_unique UNIQUE (token_hash),
    CONSTRAINT ics_tokens_workspace_id_user_id_unique UNIQUE (workspace_id, user_id),
    CONSTRAINT ics_tokens_workspace_user_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id)
        ON DELETE CASCADE
);

CREATE INDEX ics_tokens_expires_at_idx ON fvoci.ics_tokens (expires_at);

ALTER TABLE fvoci.ics_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.ics_tokens FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.ics_tokens
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    )
    WITH CHECK (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );
