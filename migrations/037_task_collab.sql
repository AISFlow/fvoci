-- Task collaboration rooms (source `task_states`, `task_origins`, revisions for
-- tasks). The task collab tables mirror the document ones (004 document_states +
-- 005 document_collab_updates / document_collab_op_receipts) one to one; the
-- document tables are unchanged. Every task table cascades with its task so a
-- task purge removes its CRDT state, tail and receipts.

-- Derived body projection of the task CRDT (source tasks.text / tasks.chosung).
ALTER TABLE fvoci.tasks
    ADD COLUMN text text NOT NULL DEFAULT '',
    ADD COLUMN chosung text NOT NULL DEFAULT '';

CREATE TABLE fvoci.task_states (
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    state bytea NOT NULL,
    encoding smallint NOT NULL DEFAULT 1,
    compacted_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    writer_generation bigint NOT NULL DEFAULT 0,
    snapshot_cutoff_seq bigint NOT NULL DEFAULT 0,
    tail_seq bigint NOT NULL DEFAULT 0,
    CONSTRAINT task_states_pkey PRIMARY KEY (workspace_id, task_id),
    CONSTRAINT task_states_encoding_check CHECK (encoding = 1),
    CONSTRAINT task_states_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE
);

ALTER TABLE fvoci.task_states ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_states FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.task_states
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.task_collab_updates (
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    seq bigint NOT NULL,
    op_id uuid NOT NULL,
    payload bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT task_collab_updates_pkey PRIMARY KEY (workspace_id, task_id, seq),
    CONSTRAINT task_collab_updates_op_unique UNIQUE (workspace_id, task_id, op_id),
    CONSTRAINT task_collab_updates_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT task_collab_updates_payload_len_check CHECK (
        octet_length(payload) >= 1 AND octet_length(payload) <= 8388608
    )
);

ALTER TABLE fvoci.task_collab_updates ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_collab_updates FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.task_collab_updates
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.task_collab_op_receipts (
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    op_id uuid NOT NULL,
    seq bigint NOT NULL,
    payload_len bigint NOT NULL,
    payload_sha256 bytea NOT NULL,
    actor_user_id uuid NOT NULL REFERENCES fvoci.users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT task_collab_op_receipts_pkey PRIMARY KEY (workspace_id, task_id, op_id),
    CONSTRAINT task_collab_op_receipts_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT task_collab_op_receipts_payload_len_check CHECK (
        payload_len >= 1 AND payload_len <= 8388608
    ),
    CONSTRAINT task_collab_op_receipts_sha256_len_check CHECK (
        octet_length(payload_sha256) = 32
    )
);

CREATE INDEX task_collab_op_receipts_lookup_idx
    ON fvoci.task_collab_op_receipts (workspace_id, task_id, seq);

ALTER TABLE fvoci.task_collab_op_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_collab_op_receipts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.task_collab_op_receipts
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

-- Source revisions_target_kind_check: document and task revisions share the table.
ALTER TABLE fvoci.revisions DROP CONSTRAINT revisions_target_kind_check;
ALTER TABLE fvoci.revisions
    ADD CONSTRAINT revisions_target_kind_check CHECK (target_kind IN ('document', 'task'));

-- Source task_origins: the task created from a document block (POST documents/:id/tasks).
CREATE TABLE fvoci.task_origins (
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    document_id uuid NOT NULL,
    request_id uuid NOT NULL,
    request_hash text NOT NULL,
    anchor text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT task_origins_workspace_id_task_id_pk PRIMARY KEY (workspace_id, task_id),
    CONSTRAINT task_origins_workspace_document_request_unique
        UNIQUE (workspace_id, document_id, request_id),
    CONSTRAINT task_origins_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT task_origins_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT task_origins_anchor_length_check
        CHECK (anchor IS NULL OR char_length(anchor) <= 200)
);

CREATE INDEX task_origins_document_task_idx
    ON fvoci.task_origins (workspace_id, document_id, task_id);

ALTER TABLE fvoci.task_origins ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_origins FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.task_origins
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));
