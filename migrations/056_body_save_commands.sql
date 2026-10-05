-- One OFF body save is an immutable command binding. Live FKs are cleared on
-- purge; the original target/result identity survives and cannot be reused.
CREATE TABLE fvoci.body_save_commands (
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces(id) ON DELETE CASCADE,
    command_id uuid NOT NULL,
    actor_user_id uuid NOT NULL REFERENCES fvoci.users(id) ON DELETE CASCADE,
    credential_id uuid NOT NULL,
    target_kind text NOT NULL CHECK (target_kind IN ('document','task')),
    target_id uuid NOT NULL,
    document_id uuid,
    task_id uuid,
    expected_tail_seq bigint NOT NULL CHECK (expected_tail_seq >= 0 AND expected_tail_seq < 9223372036854775807),
    committed_tail_seq bigint NOT NULL CHECK (committed_tail_seq = expected_tail_seq + 1),
    request_hash text NOT NULL CHECK (request_hash ~ '^[0-9a-f]{64}$'),
    payload_hash bytea NOT NULL CHECK (octet_length(payload_hash)=32),
    result_json jsonb NOT NULL CHECK (jsonb_typeof(result_json)='object'),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id,command_id),
    CHECK ((target_kind='document' AND task_id IS NULL AND (document_id IS NULL OR document_id=target_id))
        OR (target_kind='task' AND document_id IS NULL AND (task_id IS NULL OR task_id=target_id))),
    FOREIGN KEY (workspace_id,document_id) REFERENCES fvoci.documents(workspace_id,id) ON DELETE SET NULL(document_id),
    FOREIGN KEY (workspace_id,task_id) REFERENCES fvoci.tasks(workspace_id,id) ON DELETE SET NULL(task_id)
);
ALTER TABLE fvoci.body_save_commands ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.body_save_commands FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.body_save_commands
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id=(SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id=(SELECT public.app_tenant_id()));
