-- Owner-private Zotero metadata. Secrets are a separate nonportable row.
-- No remote tombstone/disconnect can cascade into ordinary authored entities.
CREATE TABLE fvoci.zotero_connectors (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces(id) ON DELETE CASCADE,
    owner_user_id uuid NOT NULL REFERENCES fvoci.users(id) ON DELETE CASCADE,
    library_type text NOT NULL CHECK (library_type IN ('user','group')),
    remote_library_id bigint NOT NULL CHECK (remote_library_id > 0),
    library_url text NOT NULL CHECK (octet_length(library_url) <= 1024),
    state text NOT NULL DEFAULT 'connected' CHECK (state IN ('connected','disconnected','denied')),
    generation bigint NOT NULL DEFAULT 1 CHECK (generation > 0),
    completed_version bigint NOT NULL DEFAULT 0 CHECK (completed_version >= 0),
    progress_version bigint CHECK (progress_version >= 0),
    committed_pages integer NOT NULL DEFAULT 0 CHECK (committed_pages >= 0),
    retry_at timestamptz,
    reconciliation_required boolean NOT NULL DEFAULT true,
    sync_id uuid,
    sync_expires_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, owner_user_id, library_type, remote_library_id),
    UNIQUE (workspace_id, owner_user_id, id),
    CHECK ((sync_id IS NULL) = (sync_expires_at IS NULL))
);
CREATE TABLE fvoci.zotero_credentials (
    connector_id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    owner_user_id uuid NOT NULL,
    sealed_key text NOT NULL CHECK (sealed_key LIKE 'enc:v2:%'),
    FOREIGN KEY (workspace_id, owner_user_id, connector_id)
        REFERENCES fvoci.zotero_connectors(workspace_id, owner_user_id, id) ON DELETE CASCADE
);
CREATE TABLE fvoci.zotero_references (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    owner_user_id uuid NOT NULL,
    connector_id uuid NOT NULL,
    document_id uuid,
    item_key text NOT NULL CHECK (item_key ~ '^[23456789ABCDEFGHIJKLMNPQRSTUVWXYZ]{8}$'),
    remote_version bigint NOT NULL CHECK (remote_version >= 0),
    local_version bigint NOT NULL DEFAULT 1 CHECK (local_version > 0),
    bibliography jsonb NOT NULL CHECK (jsonb_typeof(bibliography) = 'object'),
    return_url text NOT NULL CHECK (octet_length(return_url) <= 2048),
    availability text NOT NULL CHECK (availability IN ('available','trashed','deleted','excluded')),
    UNIQUE (workspace_id, owner_user_id, connector_id, item_key),
    UNIQUE (workspace_id, owner_user_id, connector_id, id),
    UNIQUE (workspace_id, document_id),
    FOREIGN KEY (workspace_id, owner_user_id, connector_id)
        REFERENCES fvoci.zotero_connectors(workspace_id, owner_user_id, id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, document_id) REFERENCES fvoci.documents(workspace_id,id)
        ON DELETE SET NULL (document_id)
);
CREATE TABLE fvoci.zotero_collections (
    workspace_id uuid NOT NULL,
    owner_user_id uuid NOT NULL,
    connector_id uuid NOT NULL,
    collection_key text NOT NULL CHECK (collection_key ~ '^[23456789ABCDEFGHIJKLMNPQRSTUVWXYZ]{8}$'),
    remote_version bigint NOT NULL CHECK (remote_version >= 0),
    name text NOT NULL CHECK (octet_length(name) <= 4096),
    parent_key text,
    availability text NOT NULL DEFAULT 'available' CHECK (availability IN ('available','deleted')),
    PRIMARY KEY (workspace_id, owner_user_id, connector_id, collection_key),
    FOREIGN KEY (workspace_id, owner_user_id, connector_id)
        REFERENCES fvoci.zotero_connectors(workspace_id, owner_user_id, id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, owner_user_id, connector_id, parent_key)
        REFERENCES fvoci.zotero_collections(workspace_id, owner_user_id, connector_id, collection_key)
        DEFERRABLE INITIALLY DEFERRED,
    CHECK (parent_key IS NULL OR parent_key <> collection_key)
);
CREATE TABLE fvoci.zotero_memberships (
    workspace_id uuid NOT NULL,
    owner_user_id uuid NOT NULL,
    connector_id uuid NOT NULL,
    reference_id uuid NOT NULL,
    collection_key text NOT NULL,
    PRIMARY KEY (workspace_id, owner_user_id, connector_id, reference_id, collection_key),
    FOREIGN KEY (workspace_id, owner_user_id, connector_id, reference_id)
        REFERENCES fvoci.zotero_references(workspace_id, owner_user_id, connector_id, id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, owner_user_id, connector_id, collection_key)
        REFERENCES fvoci.zotero_collections(workspace_id, owner_user_id, connector_id, collection_key) ON DELETE CASCADE
);
CREATE TABLE fvoci.zotero_links (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    owner_user_id uuid NOT NULL,
    connector_id uuid NOT NULL,
    reference_id uuid NOT NULL,
    document_id uuid,
    task_id uuid,
    anchor text NOT NULL DEFAULT '' CHECK (char_length(anchor) <= 256),
    CHECK ((document_id IS NULL) <> (task_id IS NULL)),
    UNIQUE NULLS NOT DISTINCT (workspace_id, reference_id, document_id, task_id, anchor),
    FOREIGN KEY (workspace_id, owner_user_id, connector_id, reference_id)
        REFERENCES fvoci.zotero_references(workspace_id, owner_user_id, connector_id, id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, document_id) REFERENCES fvoci.documents(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, task_id) REFERENCES fvoci.tasks(workspace_id,id) ON DELETE CASCADE
);
-- Metadata is tenant AND owner scoped even if the application forgets a filter.
-- Existing system context is reserved for operator verification/rotation.
DO $$
DECLARE table_name text;
BEGIN
    FOREACH table_name IN ARRAY ARRAY['zotero_connectors','zotero_credentials','zotero_references','zotero_collections','zotero_memberships','zotero_links'] LOOP
        EXECUTE format('ALTER TABLE fvoci.%I ENABLE ROW LEVEL SECURITY',table_name);
        EXECUTE format('ALTER TABLE fvoci.%I FORCE ROW LEVEL SECURITY',table_name);
        EXECUTE format('CREATE POLICY owner_tenant_isolation ON fvoci.%I AS PERMISSIVE FOR ALL TO public USING ((workspace_id = (SELECT public.app_tenant_id()) AND owner_user_id = (SELECT public.app_self_user_id())) OR (SELECT public.app_system_ctx_on())) WITH CHECK ((workspace_id = (SELECT public.app_tenant_id()) AND owner_user_id = (SELECT public.app_self_user_id())) OR (SELECT public.app_system_ctx_on()))',table_name);
    END LOOP;
END $$;
