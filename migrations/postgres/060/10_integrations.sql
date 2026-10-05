-- FVOCI PostgreSQL baseline fvoci-postgres-060, step 10: integrations.
-- Outgoing webhooks and their delivery ledger, the GitHub App link, and the
-- owner-private read-only Zotero mirror with its separately sealed credential.
--
-- webhooks.secret holds the signing secret sealed at rest
-- (enc:v2:<kid>:<base64url(iv||tag||ciphertext)>, AAD bound to the row), never
-- plaintext. webhook_deliveries is the per-target retry ledger fanned out by the
-- `webhooks` outbox consumer. github_deliveries dedupes inbound GitHub
-- deliveries by X-GitHub-Delivery.

CREATE TABLE fvoci.webhooks (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    url text NOT NULL,
    secret text NOT NULL,
    events text[] NOT NULL,
    created_by uuid NOT NULL REFERENCES fvoci.users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT webhooks_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT webhooks_events_check CHECK (cardinality(events) BETWEEN 1 AND 64),
    CONSTRAINT webhooks_url_check CHECK (char_length(url) BETWEEN 1 AND 2048),
    CONSTRAINT webhooks_secret_check CHECK (secret LIKE 'enc:v2:%')
);

CREATE INDEX webhooks_created_by_idx ON fvoci.webhooks (created_by);

ALTER TABLE fvoci.webhooks ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.webhooks FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.webhooks
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

CREATE TABLE fvoci.webhook_deliveries (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    webhook_id uuid NOT NULL,
    event_id uuid NOT NULL,
    attempt integer NOT NULL DEFAULT 0,
    status text NOT NULL,
    http_status integer,
    next_attempt_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT webhook_deliveries_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT webhook_deliveries_webhook_event_unique UNIQUE (webhook_id, event_id),
    CONSTRAINT webhook_deliveries_status_check CHECK (status IN ('pending', 'delivered', 'failed')),
    CONSTRAINT webhook_deliveries_attempt_check CHECK (attempt >= 0),
    CONSTRAINT webhook_deliveries_pending_check
        CHECK ((status = 'pending') = (next_attempt_at IS NOT NULL)),
    CONSTRAINT webhook_deliveries_workspace_webhook_fk
        FOREIGN KEY (workspace_id, webhook_id)
        REFERENCES fvoci.webhooks (workspace_id, id) ON DELETE CASCADE
);

CREATE INDEX webhook_deliveries_pending_next_attempt_at_idx
    ON fvoci.webhook_deliveries (next_attempt_at)
    WHERE status = 'pending';
CREATE INDEX webhook_deliveries_settled_created_at_idx
    ON fvoci.webhook_deliveries (created_at)
    WHERE status IN ('delivered', 'failed');

ALTER TABLE fvoci.webhook_deliveries ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.webhook_deliveries FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.webhook_deliveries
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

CREATE TABLE fvoci.github_installations (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    installation_id text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT github_installations_installation_id_unique UNIQUE (installation_id),
    CONSTRAINT github_installations_workspace_id_unique UNIQUE (workspace_id),
    CONSTRAINT github_installations_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT github_installations_installation_id_check
        CHECK (installation_id ~ '^[0-9]{1,20}$')
);

ALTER TABLE fvoci.github_installations ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.github_installations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.github_installations
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

-- Pending install round trips: the hash of the nonce carried in the signed
-- state, bound to the admin user and session that started it. The callback
-- deletes the row (single use) and must present the same session.
CREATE TABLE fvoci.github_install_states (
    nonce_hash text PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    session_id uuid NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT github_install_states_nonce_hash_check CHECK (nonce_hash ~ '^[0-9a-f]{64}$')
);

CREATE INDEX github_install_states_workspace_id_idx ON fvoci.github_install_states (workspace_id);
CREATE INDEX github_install_states_expires_at_idx ON fvoci.github_install_states (expires_at);

ALTER TABLE fvoci.github_install_states ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.github_install_states FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.github_install_states
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

CREATE TABLE fvoci.github_issue_links (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    repo text NOT NULL,
    issue_number integer NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT github_issue_links_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT github_issue_links_task_unique UNIQUE (workspace_id, task_id),
    CONSTRAINT github_issue_links_issue_unique UNIQUE (workspace_id, repo, issue_number),
    CONSTRAINT github_issue_links_issue_number_check CHECK (issue_number > 0),
    CONSTRAINT github_issue_links_repo_check CHECK (char_length(repo) BETWEEN 3 AND 200),
    CONSTRAINT github_issue_links_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id) ON DELETE CASCADE
);

ALTER TABLE fvoci.github_issue_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.github_issue_links FORCE ROW LEVEL SECURITY;
-- Source policy has no system bypass: every access is tenant scoped.
CREATE POLICY tenant_isolation ON fvoci.github_issue_links
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.github_deliveries (
    delivery_id uuid PRIMARY KEY,
    processed_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX github_deliveries_processed_at_idx ON fvoci.github_deliveries (processed_at);

ALTER TABLE fvoci.github_deliveries ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.github_deliveries FORCE ROW LEVEL SECURITY;
CREATE POLICY system_only ON fvoci.github_deliveries
    AS PERMISSIVE FOR ALL TO public
    USING ((SELECT public.app_system_ctx_on()));

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
ALTER TABLE fvoci.zotero_connectors ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.zotero_connectors FORCE ROW LEVEL SECURITY;
CREATE POLICY owner_tenant_isolation ON fvoci.zotero_connectors AS PERMISSIVE FOR ALL TO public USING ((workspace_id = (SELECT public.app_tenant_id()) AND owner_user_id = (SELECT public.app_self_user_id())) OR (SELECT public.app_system_ctx_on())) WITH CHECK ((workspace_id = (SELECT public.app_tenant_id()) AND owner_user_id = (SELECT public.app_self_user_id())) OR (SELECT public.app_system_ctx_on()));
ALTER TABLE fvoci.zotero_credentials ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.zotero_credentials FORCE ROW LEVEL SECURITY;
CREATE POLICY owner_tenant_isolation ON fvoci.zotero_credentials AS PERMISSIVE FOR ALL TO public USING ((workspace_id = (SELECT public.app_tenant_id()) AND owner_user_id = (SELECT public.app_self_user_id())) OR (SELECT public.app_system_ctx_on())) WITH CHECK ((workspace_id = (SELECT public.app_tenant_id()) AND owner_user_id = (SELECT public.app_self_user_id())) OR (SELECT public.app_system_ctx_on()));
ALTER TABLE fvoci.zotero_references ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.zotero_references FORCE ROW LEVEL SECURITY;
CREATE POLICY owner_tenant_isolation ON fvoci.zotero_references AS PERMISSIVE FOR ALL TO public USING ((workspace_id = (SELECT public.app_tenant_id()) AND owner_user_id = (SELECT public.app_self_user_id())) OR (SELECT public.app_system_ctx_on())) WITH CHECK ((workspace_id = (SELECT public.app_tenant_id()) AND owner_user_id = (SELECT public.app_self_user_id())) OR (SELECT public.app_system_ctx_on()));
ALTER TABLE fvoci.zotero_collections ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.zotero_collections FORCE ROW LEVEL SECURITY;
CREATE POLICY owner_tenant_isolation ON fvoci.zotero_collections AS PERMISSIVE FOR ALL TO public USING ((workspace_id = (SELECT public.app_tenant_id()) AND owner_user_id = (SELECT public.app_self_user_id())) OR (SELECT public.app_system_ctx_on())) WITH CHECK ((workspace_id = (SELECT public.app_tenant_id()) AND owner_user_id = (SELECT public.app_self_user_id())) OR (SELECT public.app_system_ctx_on()));
ALTER TABLE fvoci.zotero_memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.zotero_memberships FORCE ROW LEVEL SECURITY;
CREATE POLICY owner_tenant_isolation ON fvoci.zotero_memberships AS PERMISSIVE FOR ALL TO public USING ((workspace_id = (SELECT public.app_tenant_id()) AND owner_user_id = (SELECT public.app_self_user_id())) OR (SELECT public.app_system_ctx_on())) WITH CHECK ((workspace_id = (SELECT public.app_tenant_id()) AND owner_user_id = (SELECT public.app_self_user_id())) OR (SELECT public.app_system_ctx_on()));
ALTER TABLE fvoci.zotero_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.zotero_links FORCE ROW LEVEL SECURITY;
CREATE POLICY owner_tenant_isolation ON fvoci.zotero_links AS PERMISSIVE FOR ALL TO public USING ((workspace_id = (SELECT public.app_tenant_id()) AND owner_user_id = (SELECT public.app_self_user_id())) OR (SELECT public.app_system_ctx_on())) WITH CHECK ((workspace_id = (SELECT public.app_tenant_id()) AND owner_user_id = (SELECT public.app_self_user_id())) OR (SELECT public.app_system_ctx_on()));
