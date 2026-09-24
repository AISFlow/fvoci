CREATE TABLE fvoci.attachments (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    document_id uuid NOT NULL,
    uploader_id uuid NOT NULL REFERENCES fvoci.users (id),
    status text NOT NULL,
    name text NOT NULL,
    mime text NOT NULL DEFAULT 'application/octet-stream',
    declared_mime text,
    size_bytes bigint,
    reserved_size_bytes bigint NOT NULL,
    storage_key text NOT NULL,
    image boolean NOT NULL DEFAULT false,
    variants jsonb NOT NULL DEFAULT '{}'::jsonb,
    extract_text text NOT NULL DEFAULT '',
    extract_status text NOT NULL DEFAULT 'skipped',
    scan_status text NOT NULL DEFAULT 'skipped',
    upload_meta jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    CONSTRAINT attachments_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT attachments_storage_key_unique UNIQUE (storage_key),
    CONSTRAINT attachments_status_check CHECK (status IN ('uploading', 'assembling', 'stored')),
    CONSTRAINT attachments_scan_status_check CHECK (scan_status IN ('skipped', 'clean', 'infected')),
    CONSTRAINT attachments_extract_status_check CHECK (
        extract_status IN (
            'pending', 'skipped', 'ok', 'empty', 'partial', 'unsupported',
            'corrupt', 'resource_limit', 'worker_failure'
        )
    ),
    CONSTRAINT attachments_reserved_size_check CHECK (
        reserved_size_bytes > 0 AND reserved_size_bytes <= 9007199254740991
    ),
    CONSTRAINT attachments_stored_size_check CHECK (
        (
            status = 'stored'
            AND size_bytes IS NOT NULL
            AND size_bytes = reserved_size_bytes
            AND completed_at IS NOT NULL
        )
        OR (
            status <> 'stored'
            AND size_bytes IS NULL
            AND completed_at IS NULL
        )
    ),
    CONSTRAINT attachments_workspace_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id)
);

CREATE INDEX attachments_workspace_id_document_id_idx
    ON fvoci.attachments (workspace_id, document_id);
CREATE INDEX attachments_uploader_id_idx ON fvoci.attachments (uploader_id);
CREATE INDEX attachments_uploading_created_at_idx
    ON fvoci.attachments (status, created_at)
    WHERE status IN ('uploading', 'assembling');
CREATE INDEX attachments_uploader_id_stored_idx
    ON fvoci.attachments (uploader_id, created_at, id)
    WHERE status = 'stored';

ALTER TABLE fvoci.attachments ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.attachments
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()));
