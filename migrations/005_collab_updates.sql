ALTER TABLE fvoci.document_states
    ADD COLUMN writer_generation bigint NOT NULL DEFAULT 0,
    ADD COLUMN snapshot_cutoff_seq bigint NOT NULL DEFAULT 0,
    ADD COLUMN tail_seq bigint NOT NULL DEFAULT 0;

CREATE TABLE fvoci.document_collab_updates (
    workspace_id uuid NOT NULL,
    document_id uuid NOT NULL,
    seq bigint NOT NULL,
    op_id uuid NOT NULL,
    payload bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT document_collab_updates_pkey PRIMARY KEY (workspace_id, document_id, seq),
    CONSTRAINT document_collab_updates_op_unique UNIQUE (workspace_id, document_id, op_id),
    CONSTRAINT document_collab_updates_workspace_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT document_collab_updates_payload_len_check CHECK (
        octet_length(payload) >= 1 AND octet_length(payload) <= 8388608
    )
);

CREATE INDEX document_collab_updates_tail_idx
    ON fvoci.document_collab_updates (workspace_id, document_id, seq);

CREATE TABLE fvoci.document_collab_op_receipts (
    workspace_id uuid NOT NULL,
    document_id uuid NOT NULL,
    op_id uuid NOT NULL,
    seq bigint NOT NULL,
    payload_len bigint NOT NULL,
    payload_sha256 bytea NOT NULL,
    actor_user_id uuid NOT NULL REFERENCES fvoci.users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT document_collab_op_receipts_pkey PRIMARY KEY (workspace_id, document_id, op_id),
    CONSTRAINT document_collab_op_receipts_workspace_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT document_collab_op_receipts_payload_len_check CHECK (
        payload_len >= 1 AND payload_len <= 8388608
    ),
    CONSTRAINT document_collab_op_receipts_sha256_len_check CHECK (
        octet_length(payload_sha256) = 32
    )
);

CREATE INDEX document_collab_op_receipts_lookup_idx
    ON fvoci.document_collab_op_receipts (workspace_id, document_id, seq);

ALTER TABLE fvoci.document_collab_op_receipts ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.document_collab_op_receipts
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()));

ALTER TABLE fvoci.document_collab_updates ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.document_collab_updates
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()));
