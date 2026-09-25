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
