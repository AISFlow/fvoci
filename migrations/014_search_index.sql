CREATE TABLE fvoci.attachment_text (
    workspace_id uuid NOT NULL,
    attachment_id uuid NOT NULL,
    chunk_no integer NOT NULL,
    start_offset integer NOT NULL,
    end_offset integer NOT NULL,
    text text NOT NULL,
    chosung text NOT NULL DEFAULT '',
    status text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, attachment_id, chunk_no),
    CONSTRAINT attachment_text_chunk_no_check CHECK (chunk_no >= 0),
    CONSTRAINT attachment_text_offsets_check CHECK (
        start_offset >= 0 AND end_offset >= start_offset
    ),
    CONSTRAINT attachment_text_status_check CHECK (status IN (
        'ok', 'skipped', 'empty', 'partial', 'unsupported', 'corrupt',
        'resource_limit', 'worker_failure'
    )),
    CONSTRAINT attachment_text_parent_fk
        FOREIGN KEY (workspace_id, attachment_id)
        REFERENCES fvoci.attachments (workspace_id, id)
        ON DELETE CASCADE
);

CREATE INDEX attachment_text_workspace_attachment_idx
    ON fvoci.attachment_text (workspace_id, attachment_id);

ALTER TABLE fvoci.attachment_text ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.attachment_text FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.attachment_text
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));
