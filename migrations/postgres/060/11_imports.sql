-- FVOCI PostgreSQL baseline fvoci-postgres-060, step 11: imports and command receipts.
-- Durable import jobs (office-file, notion-zip and native-archive bindings),
-- deferred event publication for imports, and the personal-input / personal-
-- transfer command receipts with their purge-retirement FK behaviour.

-- Import jobs. markdown-zip runs inside the request; office-file, notion-zip and
-- native-archive are durable: the decoded upload is stored on the row (bounded
-- to 64 MiB) and cleared at the terminal transition, so any replica can claim
-- the job after a restart. Claims use FOR UPDATE SKIP LOCKED plus a lease token
-- that fences every worker write; created_refs is the write-ahead list of rows
-- the run created, compensated by the runner or the orphan sweep. Native
-- selective archives bind the initiating actor, request and immutable hash.
CREATE TABLE fvoci.import_jobs (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    created_by uuid NOT NULL REFERENCES fvoci.users (id),
    session_id uuid,
    source text NOT NULL,
    status text NOT NULL,
    file_name text,
    project_id uuid,
    payload bytea,
    created_refs jsonb NOT NULL DEFAULT '{"documentIds":[],"taskIds":[],"storedKeys":[]}'::jsonb,
    lease_token uuid,
    lease_until timestamptz,
    attempts smallint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    native_request_id uuid,
    native_archive_hash text,
    native_result jsonb,
    native_diagnostic text,
    CONSTRAINT import_jobs_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT import_jobs_status_check CHECK (status IN ('pending', 'running', 'completed', 'failed')),
    CONSTRAINT import_jobs_file_name_check
        CHECK (file_name IS NULL OR char_length(file_name) BETWEEN 1 AND 255),
    CONSTRAINT import_jobs_payload_size_check
        CHECK (payload IS NULL OR octet_length(payload) BETWEEN 1 AND 67108864),
    CONSTRAINT import_jobs_payload_terminal_check
        CHECK (status IN ('pending', 'running') OR payload IS NULL),
    CONSTRAINT import_jobs_lease_check CHECK ((lease_token IS NULL) = (lease_until IS NULL)),
    CONSTRAINT import_jobs_attempts_check CHECK (attempts BETWEEN 0 AND 2),
    CONSTRAINT import_jobs_refs_check CHECK (
        jsonb_typeof(created_refs -> 'documentIds') = 'array'
        AND jsonb_typeof(created_refs -> 'taskIds') = 'array'
        AND jsonb_typeof(created_refs -> 'storedKeys') = 'array'
    ),
    CONSTRAINT import_jobs_source_check
        CHECK (source IN ('markdown-zip','office-file','notion-zip','native-archive')),
    CONSTRAINT import_jobs_native_binding_check CHECK (
        (source = 'native-archive' AND native_request_id IS NOT NULL
            AND native_archive_hash IS NOT NULL AND native_archive_hash ~ '^[0-9a-f]{64}$')
        OR (source <> 'native-archive' AND native_request_id IS NULL
            AND native_archive_hash IS NULL AND native_result IS NULL AND native_diagnostic IS NULL)
    ),
    -- Source authors are inert provenance, never FK principals or grants.
    CONSTRAINT import_jobs_native_result_size_check
        CHECK (native_result IS NULL OR octet_length(native_result::text) <= 1048576)
);

CREATE INDEX import_jobs_lease_idx ON fvoci.import_jobs (status, lease_until);
CREATE INDEX import_jobs_workspace_id_idx ON fvoci.import_jobs (workspace_id);
CREATE UNIQUE INDEX import_jobs_native_request_unique
    ON fvoci.import_jobs(workspace_id,created_by,native_request_id)
    WHERE source='native-archive';

ALTER TABLE fvoci.import_jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.import_jobs FORCE ROW LEVEL SECURITY;
-- Tenant rows, or the system context used only by the cross-tenant claim and
-- orphan sweep.
CREATE POLICY tenant_isolation ON fvoci.import_jobs
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

-- Deferred event publication for imports. The outbox relay picks committed
-- events up within a second, so compensating a failed import cannot take back
-- events that consumers already saw. While a transaction sets
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

-- Receipt for the confirmed personal-to-team command's real retry consumer.
-- Result IDs are locators, not FK authority: a same-ID move removes source
-- rows, and later target purge must retire the result without recreating it.
CREATE TABLE fvoci.personal_transfer_commands (
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces(id) ON DELETE CASCADE,
    actor_user_id uuid NOT NULL REFERENCES fvoci.users(id) ON DELETE CASCADE,
    request_id uuid NOT NULL,
    session_id uuid NOT NULL,
    request_hash text NOT NULL CHECK (request_hash COLLATE "C" ~ '^[0-9a-f]{64}$'),
    action text NOT NULL CHECK (action IN ('copy', 'move')),
    destination_workspace_id uuid NOT NULL,
    destination_project_id uuid NOT NULL,
    document_id uuid NOT NULL,
    document_number integer NOT NULL CHECK (document_number > 0),
    task_id uuid,
    task_number integer,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, actor_user_id, request_id),
    CHECK ((task_id IS NULL) = (task_number IS NULL)),
    CHECK (task_number IS NULL OR task_number > 0),
    CHECK (workspace_id <> destination_workspace_id)
);
ALTER TABLE fvoci.personal_transfer_commands ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.personal_transfer_commands FORCE ROW LEVEL SECURITY;
-- Append-only, like the capture receipt. No UPDATE/DELETE policy.
CREATE POLICY personal_transfer_read ON fvoci.personal_transfer_commands
    FOR SELECT TO public USING (workspace_id = (SELECT public.app_tenant_id()));
CREATE POLICY personal_transfer_append ON fvoci.personal_transfer_commands
    FOR INSERT TO public WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));
