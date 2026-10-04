-- Native selective archives use existing durable import leases; completed
-- results remain bound to the initiating actor, target and immutable hash.
ALTER TABLE fvoci.import_jobs DROP CONSTRAINT import_jobs_source_check;
ALTER TABLE fvoci.import_jobs ADD CONSTRAINT import_jobs_source_check
    CHECK (source IN ('markdown-zip','office-file','notion-zip','native-archive'));
ALTER TABLE fvoci.import_jobs
    ADD COLUMN native_request_id uuid,
    ADD COLUMN native_archive_hash text,
    ADD COLUMN native_result jsonb,
    ADD COLUMN native_diagnostic text;
ALTER TABLE fvoci.import_jobs ADD CONSTRAINT import_jobs_native_binding_check CHECK (
    (source = 'native-archive' AND native_request_id IS NOT NULL
        AND native_archive_hash IS NOT NULL AND native_archive_hash ~ '^[0-9a-f]{64}$')
    OR (source <> 'native-archive' AND native_request_id IS NULL
        AND native_archive_hash IS NULL AND native_result IS NULL AND native_diagnostic IS NULL)
);
CREATE UNIQUE INDEX import_jobs_native_request_unique
    ON fvoci.import_jobs(workspace_id,created_by,native_request_id)
    WHERE source='native-archive';
-- Source authors are inert provenance, never FK principals or grants.
ALTER TABLE fvoci.import_jobs ADD CONSTRAINT import_jobs_native_result_size_check
    CHECK (native_result IS NULL OR octet_length(native_result::text) <= 1048576);
