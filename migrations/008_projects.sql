CREATE TABLE fvoci.projects (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    key text NOT NULL,
    name text NOT NULL,
    description text,
    icon text,
    visibility text NOT NULL,
    root_document_id uuid,
    status text NOT NULL DEFAULT 'active',
    next_number integer NOT NULL DEFAULT 1,
    created_by uuid NOT NULL REFERENCES fvoci.users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    CONSTRAINT projects_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT projects_workspace_id_key_unique UNIQUE (workspace_id, key),
    CONSTRAINT projects_visibility_check CHECK (visibility IN ('private', 'workspace')),
    CONSTRAINT projects_status_check CHECK (status IN ('active', 'archived')),
    CONSTRAINT projects_key_shape_check CHECK (
        key ~ '^(?!.*-\d+$)[A-Z][A-Z0-9-]{1,31}$'
    ),
    CONSTRAINT projects_key_reserved_check CHECK (
        upper(key) NOT IN (
            'WIKI', 'PROJECTS', 'SEARCH', 'MY-TASKS', 'TRASH', 'NOTIFICATIONS', 'SETTINGS', 'A'
        )
    ),
    CONSTRAINT projects_name_check CHECK (
        char_length(btrim(name)) >= 1 AND char_length(name) <= 200
    )
);

CREATE INDEX projects_workspace_id_deleted_at_idx
    ON fvoci.projects (workspace_id, deleted_at);

ALTER TABLE fvoci.projects ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.projects FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.projects
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.project_members (
    workspace_id uuid NOT NULL,
    project_id uuid NOT NULL,
    user_id uuid NOT NULL,
    role text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, project_id, user_id),
    CONSTRAINT project_members_role_check CHECK (role IN ('lead', 'member', 'viewer')),
    CONSTRAINT project_members_project_fk
        FOREIGN KEY (workspace_id, project_id)
        REFERENCES fvoci.projects (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT project_members_membership_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id)
        ON DELETE CASCADE
);

CREATE INDEX project_members_workspace_user_idx
    ON fvoci.project_members (workspace_id, user_id);

ALTER TABLE fvoci.project_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.project_members FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.project_members
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.workflows (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    project_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT workflows_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT workflows_project_fk
        FOREIGN KEY (workspace_id, project_id)
        REFERENCES fvoci.projects (workspace_id, id)
        ON DELETE CASCADE
);

ALTER TABLE fvoci.workflows ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.workflows FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.workflows
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.statuses (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    project_id uuid NOT NULL,
    workflow_id uuid NOT NULL,
    name text NOT NULL,
    category text NOT NULL,
    sort_key text NOT NULL,
    wip_limit integer,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT statuses_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT statuses_workspace_project_id_unique UNIQUE (workspace_id, project_id, id),
    CONSTRAINT statuses_category_check CHECK (
        category IN ('backlog', 'todo', 'in_progress', 'done', 'canceled')
    ),
    CONSTRAINT statuses_project_fk
        FOREIGN KEY (workspace_id, project_id)
        REFERENCES fvoci.projects (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT statuses_workflow_fk
        FOREIGN KEY (workspace_id, workflow_id)
        REFERENCES fvoci.workflows (workspace_id, id)
        ON DELETE CASCADE
);

CREATE INDEX statuses_workspace_project_idx
    ON fvoci.statuses (workspace_id, project_id, sort_key COLLATE "C");

ALTER TABLE fvoci.statuses ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.statuses FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.statuses
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.tasks (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    project_id uuid NOT NULL,
    number integer NOT NULL,
    title text NOT NULL,
    type text NOT NULL DEFAULT 'task',
    priority text NOT NULL DEFAULT 'none',
    status_id uuid NOT NULL,
    start_date date,
    due_date date,
    due_at timestamptz,
    estimate numeric,
    parent_id uuid,
    milestone_id uuid,
    recurrence jsonb,
    sort_key text NOT NULL DEFAULT 'V',
    schema_version integer NOT NULL DEFAULT 2,
    content_json jsonb NOT NULL,
    version integer NOT NULL DEFAULT 1,
    archived_at timestamptz,
    deleted_at timestamptz,
    created_by uuid NOT NULL REFERENCES fvoci.users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT tasks_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT tasks_workspace_project_number_unique UNIQUE (workspace_id, project_id, number),
    CONSTRAINT tasks_title_check CHECK (
        char_length(btrim(title)) >= 1 AND char_length(title) <= 500
    ),
    CONSTRAINT tasks_type_check CHECK (type IN ('task', 'bug', 'story', 'epic', 'subtask')),
    CONSTRAINT tasks_priority_check CHECK (
        priority IN ('none', 'low', 'medium', 'high', 'urgent')
    ),
    CONSTRAINT tasks_project_fk
        FOREIGN KEY (workspace_id, project_id)
        REFERENCES fvoci.projects (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT tasks_status_fk
        FOREIGN KEY (workspace_id, project_id, status_id)
        REFERENCES fvoci.statuses (workspace_id, project_id, id),
    CONSTRAINT tasks_parent_fk
        FOREIGN KEY (workspace_id, parent_id)
        REFERENCES fvoci.tasks (workspace_id, id)
);

CREATE INDEX tasks_workspace_project_idx
    ON fvoci.tasks (workspace_id, project_id)
    WHERE deleted_at IS NULL;

ALTER TABLE fvoci.tasks ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.tasks FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.tasks
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

ALTER TABLE fvoci.documents
    ADD CONSTRAINT documents_project_fk
    FOREIGN KEY (workspace_id, project_id)
    REFERENCES fvoci.projects (workspace_id, id);

CREATE OR REPLACE FUNCTION fvoci.assert_private_project_has_lead()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, fvoci
AS $$
DECLARE
    pid uuid;
    ws uuid;
    vis text;
    del timestamptz;
    lead_count integer;
BEGIN
    IF TG_TABLE_NAME = 'project_members' THEN
        pid := COALESCE(OLD.project_id, NEW.project_id);
        ws := COALESCE(OLD.workspace_id, NEW.workspace_id);
    ELSIF TG_TABLE_NAME = 'projects' THEN
        pid := COALESCE(OLD.id, NEW.id);
        ws := COALESCE(OLD.workspace_id, NEW.workspace_id);
    ELSE
        RETURN NULL;
    END IF;

    SELECT visibility, deleted_at INTO vis, del
    FROM fvoci.projects
    WHERE workspace_id = ws AND id = pid;

    IF NOT FOUND OR del IS NOT NULL OR vis <> 'private' THEN
        RETURN NULL;
    END IF;

    SELECT count(*) INTO lead_count
    FROM fvoci.project_members
    WHERE workspace_id = ws AND project_id = pid AND role = 'lead';

    IF lead_count = 0 THEN
        RAISE EXCEPTION 'private project requires a lead'
            USING ERRCODE = '23514';
    END IF;

    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER projects_private_lead_check
    AFTER UPDATE OF visibility ON fvoci.projects
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION fvoci.assert_private_project_has_lead();

CREATE CONSTRAINT TRIGGER project_members_private_lead_check_del
    AFTER DELETE ON fvoci.project_members
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION fvoci.assert_private_project_has_lead();

CREATE CONSTRAINT TRIGGER project_members_private_lead_check_upd
    AFTER UPDATE OF role ON fvoci.project_members
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION fvoci.assert_private_project_has_lead();
