-- An ordinary wiki create is one command, including a retry after a lost
-- response. Purge clears only the live target, retaining the original binding
-- and response; a retired command can never create a replacement document.
CREATE TABLE fvoci.wiki_create_commands (
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    command_id uuid NOT NULL,
    actor_user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    request_hash text NOT NULL CHECK (request_hash ~ '^[0-9a-f]{64}$'),
    document_id uuid,
    result_json jsonb NOT NULL CHECK (jsonb_typeof(result_json) = 'object'),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, command_id),
    FOREIGN KEY (workspace_id, document_id) REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE SET NULL (document_id)
);
ALTER TABLE fvoci.wiki_create_commands ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.wiki_create_commands FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.wiki_create_commands
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));
