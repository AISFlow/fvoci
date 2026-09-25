-- Import jobs. markdown-zip runs inside the request; office-file and
-- notion-zip are durable: the decoded upload is stored on the row (bounded to
-- 64 MiB) and cleared at the terminal transition, so any replica can claim the
-- job after a restart. Claims use FOR UPDATE SKIP LOCKED plus a lease token
-- that fences every worker write; created_refs is the write-ahead list of
-- rows the run created, compensated by the runner or the orphan sweep.
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
    CONSTRAINT import_jobs_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT import_jobs_source_check CHECK (source IN ('markdown-zip', 'office-file', 'notion-zip')),
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
    )
);

CREATE INDEX import_jobs_lease_idx ON fvoci.import_jobs (status, lease_until);
CREATE INDEX import_jobs_workspace_id_idx ON fvoci.import_jobs (workspace_id);

ALTER TABLE fvoci.import_jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.import_jobs FORCE ROW LEVEL SECURITY;
-- Same shape as the source policy: tenant rows, or the system context used
-- only by the cross-tenant claim and orphan sweep.
CREATE POLICY tenant_isolation ON fvoci.import_jobs
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );
