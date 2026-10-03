-- 034 is immutable. Existing closed/manual ranges and every legacy open row
-- remain byte-for-byte unchanged. Legacy locators reserve the actor until the
-- user explicitly closes/corrects the old row with current task Edit access.
-- Multiple pre-upgrade opens are all preserved; no winner or guessed end time.

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
INSERT INTO fvoci.task_timer_legacy_open (time_entry_id, user_id, workspace_id, task_id)
    SELECT id, user_id, workspace_id, task_id FROM fvoci.time_entries WHERE ended_at IS NULL;

CREATE TABLE fvoci.task_timer_commands (
    user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    request_id uuid NOT NULL,
    request_hash text NOT NULL CHECK (char_length(request_hash) = 64),
    run_id uuid REFERENCES fvoci.task_timer_runs (id) ON DELETE SET NULL,
    result jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
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

-- These tables contain only the actor's own stopwatch. This is a new bounded
-- self-state policy, not an OR clause on tenant/content RLS. Resource metadata
-- and all task/manual writes still use the existing tenant + task ACL checks.
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
