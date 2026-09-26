-- Deferred event publication for Notion imports (source `deferEvents` /
-- `commitImport`, #583). The outbox relay picks committed events up within a
-- second, so compensating a failed import cannot take back `task.created`,
-- `document.created` or `attachment.completed` that webhooks, GitHub sync and
-- notifications already consumed. Instead, while a transaction sets
-- `app.import_defer_job` to its running job, every event it appends is parked
-- here; the completing transaction moves them into fvoci.events in their
-- original order together with the `completed` transition, and every failure
-- path (runner, restart recovery, orphan sweep) deletes them.
CREATE TABLE fvoci.import_deferred_events (
    workspace_id uuid NOT NULL,
    import_job_id uuid NOT NULL,
    id uuid NOT NULL,
    seq bigint NOT NULL,
    actor_user_id uuid,
    verb text NOT NULL,
    target_type text,
    target_id uuid,
    payload jsonb NOT NULL,
    channel text NOT NULL,
    created_at timestamptz NOT NULL,
    PRIMARY KEY (import_job_id, id),
    CONSTRAINT import_deferred_events_job_fk
        FOREIGN KEY (workspace_id, import_job_id)
        REFERENCES fvoci.import_jobs (workspace_id, id)
        ON DELETE CASCADE
);

CREATE INDEX import_deferred_events_job_seq_idx
    ON fvoci.import_deferred_events (workspace_id, import_job_id, seq);

ALTER TABLE fvoci.import_deferred_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.import_deferred_events FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.import_deferred_events
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

-- Invoker rights: runs as the inserting role under its tenant RLS. The job
-- row is share-locked and must still be running; once the sweep or the
-- runner failed it, the event is dropped (its rows are being compensated),
-- and a sweep that locked the row first makes this wait, then drop.
CREATE FUNCTION fvoci.events_defer_import()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    job uuid := NULLIF(current_setting('app.import_defer_job', true), '')::uuid;
BEGIN
    IF job IS NULL OR NEW.workspace_id IS NULL THEN
        RETURN NEW;
    END IF;
    PERFORM 1
    FROM fvoci.import_jobs AS j
    WHERE j.workspace_id = NEW.workspace_id
      AND j.id = job
      AND j.status = 'running'
    FOR SHARE;
    IF FOUND THEN
        INSERT INTO fvoci.import_deferred_events (
            workspace_id, import_job_id, id, seq, actor_user_id, verb,
            target_type, target_id, payload, channel, created_at
        ) VALUES (
            NEW.workspace_id, job, NEW.id, NEW.seq, NEW.actor_user_id, NEW.verb,
            NEW.target_type, NEW.target_id, NEW.payload, NEW.channel, NEW.created_at
        );
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER events_defer_import
    BEFORE INSERT ON fvoci.events
    FOR EACH ROW EXECUTE FUNCTION fvoci.events_defer_import();
