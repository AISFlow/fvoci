CREATE TABLE fvoci.import_jobs (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    created_by uuid NOT NULL REFERENCES fvoci.users (id),
    source text NOT NULL,
    status text NOT NULL,
    created_refs jsonb NOT NULL DEFAULT '{"documentIds":[],"taskIds":[],"storedKeys":[]}'::jsonb,
    lease_until timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT import_jobs_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT import_jobs_source_check CHECK (source IN ('markdown-zip', 'office-file', 'notion-zip')),
    CONSTRAINT import_jobs_status_check CHECK (status IN ('pending', 'running', 'completed', 'failed'))
);

CREATE INDEX import_jobs_lease_idx ON fvoci.import_jobs (status, lease_until);
CREATE INDEX import_jobs_workspace_id_idx ON fvoci.import_jobs (workspace_id);

ALTER TABLE fvoci.import_jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.import_jobs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.import_jobs
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()));
