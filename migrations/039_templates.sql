-- Workspace document/task templates (source packages/db templates table).

CREATE TABLE fvoci.templates (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    kind text NOT NULL,
    title text NOT NULL,
    payload jsonb NOT NULL,
    created_by uuid NOT NULL REFERENCES fvoci.users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT templates_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT templates_kind_check CHECK (kind IN ('document', 'task')),
    CONSTRAINT templates_title_check CHECK (length(btrim(title)) BETWEEN 1 AND 200)
);

CREATE INDEX templates_workspace_id_idx ON fvoci.templates (workspace_id);
CREATE INDEX templates_created_by_idx ON fvoci.templates (created_by);

ALTER TABLE fvoci.templates ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.templates FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.templates
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    )
    WITH CHECK (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );
