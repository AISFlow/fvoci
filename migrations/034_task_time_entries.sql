-- Source time_entries. The task FK cascades so a task purge removes its entries
-- (the source's NO ACTION FK makes purging a task with entries fail).
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
