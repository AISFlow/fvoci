CREATE TABLE fvoci.revisions (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    target_kind text NOT NULL,
    target_id uuid NOT NULL,
    y_snapshot bytea NOT NULL,
    encoding smallint NOT NULL DEFAULT 1,
    content_json jsonb NOT NULL,
    text text NOT NULL,
    reason text NOT NULL,
    created_by uuid REFERENCES fvoci.users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT revisions_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT revisions_target_kind_check CHECK (target_kind = 'document'),
    CONSTRAINT revisions_reason_check CHECK (reason IN ('manual', 'session', 'scheduled')),
    CONSTRAINT revisions_encoding_check CHECK (encoding = 1)
);

CREATE INDEX revisions_workspace_id_target_id_created_at_id_idx
    ON fvoci.revisions (workspace_id, target_id, created_at DESC, id DESC);

ALTER TABLE fvoci.revisions ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.revisions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.revisions
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));
