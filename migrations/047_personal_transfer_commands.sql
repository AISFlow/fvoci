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
-- Append-only, like the existing capture receipt. No UPDATE/DELETE policy.
CREATE POLICY personal_transfer_read ON fvoci.personal_transfer_commands
    FOR SELECT TO public USING (workspace_id = (SELECT public.app_tenant_id()));
CREATE POLICY personal_transfer_append ON fvoci.personal_transfer_commands
    FOR INSERT TO public WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));
