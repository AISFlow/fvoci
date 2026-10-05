-- FVOCI PostgreSQL baseline fvoci-postgres-060, step 05: projects and tasks.
-- projects and their principals, workflows/statuses, tasks and task relations,
-- activity, manual time entries, task collaboration rooms and the actor-private
-- stopwatch (runs, segments, legacy-open locators, command receipts, audit).
-- task_origins (task created from a document block) is a step 06 object.

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

-- A project principal is a workspace member or a group (exactly one). Column
-- order keeps the retired lineage's catalog order (id and group_id were added
-- after the original composite key was replaced by the surrogate id).
CREATE TABLE fvoci.project_members (
    workspace_id uuid NOT NULL,
    project_id uuid NOT NULL,
    user_id uuid,
    role text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    id uuid NOT NULL,
    group_id uuid,
    CONSTRAINT project_members_pkey PRIMARY KEY (id),
    CONSTRAINT project_members_role_check CHECK (role IN ('lead', 'member', 'viewer')),
    CONSTRAINT project_members_project_fk
        FOREIGN KEY (workspace_id, project_id)
        REFERENCES fvoci.projects (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT project_members_membership_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id)
        ON DELETE CASCADE,
    CONSTRAINT project_members_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT project_members_principal_xor_check
        CHECK ((user_id IS NULL) <> (group_id IS NULL)),
    CONSTRAINT project_members_workspace_group_fk
        FOREIGN KEY (workspace_id, group_id)
        REFERENCES fvoci.groups (workspace_id, id)
        ON DELETE CASCADE
);

CREATE INDEX project_members_workspace_user_idx
    ON fvoci.project_members (workspace_id, user_id);
CREATE UNIQUE INDEX project_members_user_unique
    ON fvoci.project_members (workspace_id, project_id, user_id);
CREATE UNIQUE INDEX project_members_group_unique
    ON fvoci.project_members (workspace_id, project_id, group_id);
CREATE INDEX project_members_workspace_id_group_id_idx
    ON fvoci.project_members (workspace_id, group_id)
    WHERE group_id IS NOT NULL;

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

-- estimate is an unbounded exact decimal; estimate_unit is NULL until an explicit
-- minute command establishes it (then estimate must be a non-negative integer
-- within int4). text/chosung are the derived body projection of the task CRDT.
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
    text text NOT NULL DEFAULT '',
    chosung text NOT NULL DEFAULT '',
    estimate_unit text,
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
        REFERENCES fvoci.tasks (workspace_id, id),
    CONSTRAINT tasks_workspace_milestone_fk
        FOREIGN KEY (workspace_id, milestone_id)
        REFERENCES fvoci.milestones (workspace_id, id),
    CONSTRAINT task_estimate_explicit_minutes CHECK (
        estimate_unit IS NULL OR (
            estimate_unit = 'minutes' AND estimate IS NOT NULL
            AND estimate >= 0 AND estimate = trunc(estimate)
            AND estimate <= 2147483647
        )
    )
);

CREATE INDEX tasks_workspace_project_idx
    ON fvoci.tasks (workspace_id, project_id)
    WHERE deleted_at IS NULL;
CREATE INDEX tasks_workspace_id_milestone_id_idx
    ON fvoci.tasks (workspace_id, milestone_id);

ALTER TABLE fvoci.tasks ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.tasks FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.tasks
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

-- A private project always keeps at least one lead.
CREATE FUNCTION fvoci.assert_private_project_has_lead()
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

CREATE TABLE fvoci.task_activity (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    actor_user_id uuid REFERENCES fvoci.users (id) ON DELETE SET NULL,
    channel text NOT NULL,
    kind text NOT NULL,
    changes jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT task_activity_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT task_activity_kind_check CHECK (kind IN ('created', 'changed')),
    CONSTRAINT task_activity_channel_check
        CHECK (channel IN ('web', 'api', 'mcp', 'webhook', 'system')),
    CONSTRAINT task_activity_changes_check
        CHECK (
            jsonb_typeof(changes) = 'array'
            AND jsonb_array_length(changes) <= 14
            AND (kind = 'created' OR jsonb_array_length(changes) > 0)
        )
);

CREATE INDEX task_activity_workspace_task_created_idx
    ON fvoci.task_activity (workspace_id, task_id, created_at DESC, id DESC);

CREATE INDEX task_activity_actor_idx ON fvoci.task_activity (actor_user_id);

ALTER TABLE fvoci.task_activity ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_activity FORCE ROW LEVEL SECURITY;
-- Append-only: the app role may read and append, never rewrite or erase.
-- With RLS forced and no UPDATE/DELETE policy, both are denied; FK cascades
-- from tasks and users run as the owner and are unaffected.
CREATE POLICY task_activity_read ON fvoci.task_activity
    AS PERMISSIVE FOR SELECT TO public
    USING (workspace_id = (SELECT public.app_tenant_id()));
CREATE POLICY task_activity_append ON fvoci.task_activity
    AS PERMISSIVE FOR INSERT TO public
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

-- Manual time entries. The task FK cascades so a task purge removes its entries.
CREATE TABLE fvoci.time_entries (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    task_id uuid NOT NULL,
    user_id uuid NOT NULL REFERENCES fvoci.users (id),
    started_at timestamptz NOT NULL,
    ended_at timestamptz,
    duration_seconds integer,
    note text,
    CONSTRAINT time_entries_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT time_entries_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT time_entries_ended_after_started_check
        CHECK (ended_at IS NULL OR ended_at > started_at),
    CONSTRAINT time_entries_duration_matches_range_check CHECK (
        (ended_at IS NULL AND duration_seconds IS NULL)
        OR (
            ended_at IS NOT NULL
            AND duration_seconds = FLOOR(EXTRACT(EPOCH FROM (ended_at - started_at)))::int
            AND duration_seconds > 0
        )
    ),
    CONSTRAINT time_entries_note_length_check
        CHECK (note IS NULL OR char_length(note) <= 2000)
);

CREATE UNIQUE INDEX time_entries_one_open_per_actor
    ON fvoci.time_entries (workspace_id, user_id)
    WHERE ended_at IS NULL;

CREATE INDEX time_entries_workspace_id_task_id_started_at_idx
    ON fvoci.time_entries (workspace_id, task_id, started_at);

ALTER TABLE fvoci.time_entries ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.time_entries FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.time_entries
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

-- Task collaboration rooms mirror the document ones (step 06) one to one.
-- Every task table cascades with its task so a task purge removes its CRDT
-- state, tail and receipts.
CREATE TABLE fvoci.task_states (
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    state bytea NOT NULL,
    encoding smallint NOT NULL DEFAULT 1,
    compacted_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    writer_generation bigint NOT NULL DEFAULT 0,
    snapshot_cutoff_seq bigint NOT NULL DEFAULT 0,
    tail_seq bigint NOT NULL DEFAULT 0,
    CONSTRAINT task_states_pkey PRIMARY KEY (workspace_id, task_id),
    CONSTRAINT task_states_encoding_check CHECK (encoding = 1),
    CONSTRAINT task_states_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE
);

ALTER TABLE fvoci.task_states ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_states FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.task_states
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.task_collab_updates (
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    seq bigint NOT NULL,
    op_id uuid NOT NULL,
    payload bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT task_collab_updates_pkey PRIMARY KEY (workspace_id, task_id, seq),
    CONSTRAINT task_collab_updates_op_unique UNIQUE (workspace_id, task_id, op_id),
    CONSTRAINT task_collab_updates_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT task_collab_updates_payload_len_check CHECK (
        octet_length(payload) >= 1 AND octet_length(payload) <= 8388608
    )
);

ALTER TABLE fvoci.task_collab_updates ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_collab_updates FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.task_collab_updates
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.task_collab_op_receipts (
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    op_id uuid NOT NULL,
    seq bigint NOT NULL,
    payload_len bigint NOT NULL,
    payload_sha256 bytea NOT NULL,
    actor_user_id uuid NOT NULL REFERENCES fvoci.users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT task_collab_op_receipts_pkey PRIMARY KEY (workspace_id, task_id, op_id),
    CONSTRAINT task_collab_op_receipts_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT task_collab_op_receipts_payload_len_check CHECK (
        payload_len >= 1 AND payload_len <= 8388608
    ),
    CONSTRAINT task_collab_op_receipts_sha256_len_check CHECK (
        octet_length(payload_sha256) = 32
    )
);

CREATE INDEX task_collab_op_receipts_lookup_idx
    ON fvoci.task_collab_op_receipts (workspace_id, task_id, seq);

ALTER TABLE fvoci.task_collab_op_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_collab_op_receipts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.task_collab_op_receipts
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

-- Actor-private stopwatch. These tables contain only the actor's own stopwatch:
-- a bounded self-state policy, not an OR clause on tenant/content RLS. Resource
-- metadata and all task/manual writes still use the tenant + task ACL checks.
CREATE TABLE fvoci.task_timer_runs (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    task_id uuid NOT NULL,
    status text NOT NULL CHECK (status IN ('running', 'paused', 'stopped')),
    version integer NOT NULL CHECK (version > 0),
    started_at timestamptz NOT NULL,
    stopped_at timestamptz,
    note text CHECK (note IS NULL OR char_length(note) <= 2000),
    UNIQUE (id, user_id, workspace_id, task_id),
    FOREIGN KEY (workspace_id, task_id) REFERENCES fvoci.tasks (workspace_id, id) ON DELETE CASCADE,
    CHECK ((status = 'stopped') = (stopped_at IS NOT NULL)),
    CHECK (stopped_at IS NULL OR stopped_at >= started_at)
);
-- Paused runs reserve the same run. Starting another task requires explicit
-- stop, even in another workspace or a new session/server process.
CREATE UNIQUE INDEX task_timer_one_unfinished_per_person
    ON fvoci.task_timer_runs (user_id) WHERE status <> 'stopped';
CREATE INDEX task_timer_runs_task ON fvoci.task_timer_runs (workspace_id, task_id, user_id);

CREATE TABLE fvoci.task_timer_segments (
    id uuid PRIMARY KEY,
    run_id uuid NOT NULL,
    user_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    started_at timestamptz NOT NULL,
    ended_at timestamptz,
    time_entry_id uuid,
    FOREIGN KEY (run_id, user_id, workspace_id, task_id)
        REFERENCES fvoci.task_timer_runs (id, user_id, workspace_id, task_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, time_entry_id) REFERENCES fvoci.time_entries (workspace_id, id)
        ON DELETE SET NULL (time_entry_id),
    UNIQUE (time_entry_id),
    CHECK (ended_at IS NULL OR ended_at >= started_at),
    CHECK (time_entry_id IS NULL OR ended_at IS NOT NULL)
);
CREATE UNIQUE INDEX task_timer_one_open_segment ON fvoci.task_timer_segments (run_id)
    WHERE ended_at IS NULL;

-- Legacy open manual entries reserve the actor until explicitly closed or
-- corrected. Rows are maintained by the time_entries trigger below.
CREATE TABLE fvoci.task_timer_legacy_open (
    time_entry_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    FOREIGN KEY (workspace_id, time_entry_id) REFERENCES fvoci.time_entries (workspace_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, task_id) REFERENCES fvoci.tasks (workspace_id, id) ON DELETE CASCADE
);
CREATE INDEX task_timer_legacy_person ON fvoci.task_timer_legacy_open (user_id);

-- Command receipts. run_id is an immutable actor-private historical locator,
-- deliberately not an FK: a purged run never nulls or reconstructs it.
-- restored_from_archive is NULL for every live write and the restoring import
-- job id for a receipt imported by a native archive restore (never replayed).
CREATE TABLE fvoci.task_timer_commands (
    user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    request_id uuid NOT NULL,
    request_hash text NOT NULL CHECK (char_length(request_hash) = 64),
    run_id uuid,
    result jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    restored_from_archive uuid,
    PRIMARY KEY (user_id, request_id)
);

-- Before/after range and explicit reason are immutable, including cleanup
-- after resource revocation. No titles, document contents or credentials.
CREATE TABLE fvoci.task_timer_audit (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    request_id uuid NOT NULL,
    workspace_id uuid,
    task_id uuid,
    time_entry_id uuid,
    verb text NOT NULL,
    before_value jsonb NOT NULL,
    after_value jsonb NOT NULL,
    reason text NOT NULL CHECK (char_length(reason) BETWEEN 1 AND 2000),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX task_timer_audit_user_id_idx ON fvoci.task_timer_audit(user_id);

ALTER TABLE fvoci.task_timer_runs ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_timer_runs FORCE ROW LEVEL SECURITY;
CREATE POLICY self_timer ON fvoci.task_timer_runs
    USING (user_id = (SELECT public.app_self_user_id()))
    WITH CHECK (user_id = (SELECT public.app_self_user_id()));
ALTER TABLE fvoci.task_timer_segments ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_timer_segments FORCE ROW LEVEL SECURITY;
CREATE POLICY self_timer ON fvoci.task_timer_segments
    USING (user_id = (SELECT public.app_self_user_id()))
    WITH CHECK (user_id = (SELECT public.app_self_user_id()));
ALTER TABLE fvoci.task_timer_legacy_open ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_timer_legacy_open FORCE ROW LEVEL SECURITY;
CREATE POLICY self_timer ON fvoci.task_timer_legacy_open
    USING (user_id = (SELECT public.app_self_user_id()))
    WITH CHECK (user_id = (SELECT public.app_self_user_id()));
ALTER TABLE fvoci.task_timer_commands ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_timer_commands FORCE ROW LEVEL SECURITY;
CREATE POLICY self_timer ON fvoci.task_timer_commands
    USING (user_id = (SELECT public.app_self_user_id()))
    WITH CHECK (user_id = (SELECT public.app_self_user_id()));
ALTER TABLE fvoci.task_timer_audit ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_timer_audit FORCE ROW LEVEL SECURITY;
CREATE POLICY self_timer ON fvoci.task_timer_audit
    USING (user_id = (SELECT public.app_self_user_id()))
    WITH CHECK (user_id = (SELECT public.app_self_user_id()));

-- Invoker rights only: no tenant bypass, SECURITY DEFINER, or system context.
-- App writers must set their transaction-local self actor before open-row
-- effects. Closed historical imports keep their existing behavior.
CREATE FUNCTION fvoci.track_legacy_time_entry() RETURNS trigger
LANGUAGE plpgsql SECURITY INVOKER SET search_path = '' AS $$
BEGIN
    IF TG_OP = 'INSERT' AND NEW.ended_at IS NULL THEN
        IF NEW.user_id IS DISTINCT FROM public.app_self_user_id() THEN
            RAISE EXCEPTION 'legacy open entry requires self actor' USING ERRCODE = '42501';
        END IF;
        -- Same actual actor row as recheck_session; serializes cross-workspace
        -- opens and stopwatch starts without a new advisory namespace.
        PERFORM id FROM fvoci.users WHERE id = NEW.user_id FOR UPDATE;
        IF EXISTS (SELECT 1 FROM fvoci.task_timer_runs WHERE user_id = NEW.user_id AND status <> 'stopped')
           OR EXISTS (SELECT 1 FROM fvoci.task_timer_legacy_open WHERE user_id = NEW.user_id) THEN
            RAISE EXCEPTION 'open time entry exists' USING ERRCODE = '23505';
        END IF;
        INSERT INTO fvoci.task_timer_legacy_open (time_entry_id, user_id, workspace_id, task_id)
            VALUES (NEW.id, NEW.user_id, NEW.workspace_id, NEW.task_id);
    ELSIF TG_OP = 'UPDATE' AND OLD.ended_at IS NULL AND NEW.ended_at IS NOT NULL THEN
        DELETE FROM fvoci.task_timer_legacy_open WHERE time_entry_id = NEW.id;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER task_timer_legacy_tracking AFTER INSERT OR UPDATE ON fvoci.time_entries
    FOR EACH ROW EXECUTE FUNCTION fvoci.track_legacy_time_entry();
