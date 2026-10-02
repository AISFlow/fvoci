-- A receipt for the real personal-input retry consumer. Content and task state
-- remain ordinary entities; a purged target retires the command, never creates
-- a replacement. Only target FK columns clear on purge, preserving its key/hash.
CREATE TABLE fvoci.personal_input_commands (
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    actor_user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    request_id uuid NOT NULL,
    request_hash text NOT NULL CHECK (char_length(request_hash) = 64),
    intent text NOT NULL CHECK (intent IN ('quick', 'note', 'task')),
    document_id uuid,
    task_id uuid,
    project_id uuid,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, actor_user_id, request_id),
    FOREIGN KEY (workspace_id, document_id) REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE SET NULL (document_id),
    FOREIGN KEY (workspace_id, task_id) REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE SET NULL (task_id),
    FOREIGN KEY (workspace_id, project_id) REFERENCES fvoci.projects (workspace_id, id)
        ON DELETE SET NULL (project_id),
    CHECK (intent = 'task' OR (task_id IS NULL AND project_id IS NULL))
);
ALTER TABLE fvoci.personal_input_commands ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.personal_input_commands FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.personal_input_commands
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));
