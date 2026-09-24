CREATE TABLE fvoci.comments (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    document_id uuid,
    task_id uuid,
    parent_id uuid,
    created_by uuid NOT NULL REFERENCES fvoci.users (id),
    body text NOT NULL,
    chosung text NOT NULL DEFAULT '',
    resolved_at timestamptz,
    reactions jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT comments_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT comments_parent_xor_check CHECK (
        (document_id IS NULL) <> (task_id IS NULL)
    ),
    CONSTRAINT comments_body_check CHECK (
        char_length(btrim(body)) >= 1 AND char_length(body) <= 8000
    )
);

ALTER TABLE fvoci.comments
    ADD CONSTRAINT comments_workspace_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE CASCADE;

ALTER TABLE fvoci.comments
    ADD CONSTRAINT comments_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE;

ALTER TABLE fvoci.comments
    ADD CONSTRAINT comments_workspace_parent_fk
        FOREIGN KEY (workspace_id, parent_id)
        REFERENCES fvoci.comments (workspace_id, id)
        ON DELETE CASCADE;

CREATE INDEX comments_workspace_id_parent_id_idx
    ON fvoci.comments (workspace_id, parent_id);

CREATE INDEX comments_workspace_id_created_by_idx
    ON fvoci.comments (workspace_id, created_by, created_at, id);

CREATE INDEX comments_workspace_id_document_id_created_at_idx
    ON fvoci.comments (workspace_id, document_id, created_at, id);

CREATE INDEX comments_workspace_id_task_id_created_at_idx
    ON fvoci.comments (workspace_id, task_id, created_at, id);

ALTER TABLE fvoci.comments ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.comments FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.comments
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));
