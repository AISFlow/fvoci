CREATE TABLE fvoci.labels (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    project_id uuid NOT NULL,
    name text NOT NULL,
    color text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT labels_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT labels_workspace_project_fk
        FOREIGN KEY (workspace_id, project_id)
        REFERENCES fvoci.projects (workspace_id, id)
        ON DELETE CASCADE
);

CREATE INDEX labels_workspace_id_project_id_idx
    ON fvoci.labels (workspace_id, project_id);

ALTER TABLE fvoci.labels ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.labels FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.labels
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.task_assignees (
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    user_id uuid NOT NULL,
    PRIMARY KEY (task_id, user_id),
    CONSTRAINT task_assignees_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT task_assignees_workspace_user_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id)
        ON DELETE CASCADE
);

CREATE INDEX task_assignees_user_id_idx
    ON fvoci.task_assignees (user_id);
CREATE INDEX task_assignees_workspace_id_task_id_idx
    ON fvoci.task_assignees (workspace_id, task_id);
CREATE INDEX task_assignees_workspace_id_user_id_idx
    ON fvoci.task_assignees (workspace_id, user_id);

ALTER TABLE fvoci.task_assignees ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_assignees FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.task_assignees
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.task_labels (
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    label_id uuid NOT NULL,
    PRIMARY KEY (task_id, label_id),
    CONSTRAINT task_labels_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT task_labels_workspace_label_fk
        FOREIGN KEY (workspace_id, label_id)
        REFERENCES fvoci.labels (workspace_id, id)
        ON DELETE CASCADE
);

CREATE INDEX task_labels_workspace_id_label_id_idx
    ON fvoci.task_labels (workspace_id, label_id);
CREATE INDEX task_labels_workspace_id_task_id_idx
    ON fvoci.task_labels (workspace_id, task_id);

ALTER TABLE fvoci.task_labels ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_labels FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.task_labels
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));
