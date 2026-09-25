CREATE TABLE fvoci.milestones (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    project_id uuid NOT NULL,
    name text NOT NULL,
    due_date date,
    sort_key text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT milestones_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT milestones_workspace_project_fk
        FOREIGN KEY (workspace_id, project_id)
        REFERENCES fvoci.projects (workspace_id, id)
        ON DELETE CASCADE
);

CREATE INDEX milestones_workspace_id_project_id_idx
    ON fvoci.milestones (workspace_id, project_id);

ALTER TABLE fvoci.milestones ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.milestones FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.milestones
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

ALTER TABLE fvoci.tasks
    ADD CONSTRAINT tasks_workspace_milestone_fk
        FOREIGN KEY (workspace_id, milestone_id)
        REFERENCES fvoci.milestones (workspace_id, id);

CREATE INDEX tasks_workspace_id_milestone_id_idx
    ON fvoci.tasks (workspace_id, milestone_id);

CREATE TABLE fvoci.task_dependencies (
    workspace_id uuid NOT NULL,
    blocker_id uuid NOT NULL,
    blocked_id uuid NOT NULL,
    type text NOT NULL DEFAULT 'FS',
    lag_days integer NOT NULL DEFAULT 0,
    PRIMARY KEY (blocker_id, blocked_id),
    CONSTRAINT task_dependencies_no_self_check CHECK (blocker_id <> blocked_id),
    CONSTRAINT task_dependencies_type_check CHECK (type IN ('FS', 'SS', 'FF')),
    CONSTRAINT task_dependencies_lag_nonneg_check CHECK (lag_days >= 0),
    CONSTRAINT task_dependencies_workspace_blocker_fk
        FOREIGN KEY (workspace_id, blocker_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT task_dependencies_workspace_blocked_fk
        FOREIGN KEY (workspace_id, blocked_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE
);

CREATE INDEX task_dependencies_workspace_id_blocked_id_idx
    ON fvoci.task_dependencies (workspace_id, blocked_id);
CREATE INDEX task_dependencies_workspace_id_blocker_id_idx
    ON fvoci.task_dependencies (workspace_id, blocker_id);

ALTER TABLE fvoci.task_dependencies ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_dependencies FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.task_dependencies
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));
