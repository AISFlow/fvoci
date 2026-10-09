-- FVOCI SQLite-family new-install baseline, lineage fvoci-sqlite-060, step 10: integrations.
-- webhooks, GitHub App link and the owner-private Zotero mirror.

CREATE TABLE webhooks (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    url TEXT NOT NULL CHECK (length(url) BETWEEN 1 AND 2048),
    secret TEXT NOT NULL CHECK (substr(secret,1,7)='enc:v2:'),
    events TEXT NOT NULL CHECK (json_valid(events) AND json_type(events)='array' AND json_array_length(events) BETWEEN 1 AND 64),
    created_by BLOB NOT NULL REFERENCES users(id) CHECK (typeof(created_by)='blob' AND length(created_by)=16),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id)
) STRICT;
CREATE TABLE webhook_deliveries (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    webhook_id BLOB NOT NULL CHECK (typeof(webhook_id)='blob' AND length(webhook_id)=16),
    event_id BLOB NOT NULL CHECK (typeof(event_id)='blob' AND length(event_id)=16),
    attempt INTEGER NOT NULL DEFAULT 0 CHECK (attempt BETWEEN 0 AND 2147483647),
    status TEXT NOT NULL CHECK (status IN ('pending','delivered','failed')),
    http_status INTEGER CHECK (http_status BETWEEN -2147483648 AND 2147483647),
    next_attempt_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id), UNIQUE (webhook_id,event_id),
    CHECK ((status='pending')=(next_attempt_at IS NOT NULL)),
    FOREIGN KEY (workspace_id,webhook_id) REFERENCES webhooks(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE github_installations (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL UNIQUE REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    installation_id TEXT NOT NULL UNIQUE CHECK (length(installation_id) BETWEEN 1 AND 20 AND installation_id NOT GLOB '*[^0-9]*'),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id)
) STRICT;
CREATE TABLE github_install_states (
    nonce_hash TEXT PRIMARY KEY NOT NULL CHECK (length(nonce_hash)=64 AND nonce_hash NOT GLOB '*[^0-9a-f]*'),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    session_id BLOB NOT NULL CHECK (typeof(session_id)='blob' AND length(session_id)=16),
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE TABLE github_issue_links (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    repo TEXT NOT NULL CHECK (length(repo) BETWEEN 3 AND 200),
    issue_number INTEGER NOT NULL CHECK (issue_number BETWEEN 1 AND 2147483647),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id), UNIQUE (workspace_id,task_id), UNIQUE (workspace_id,repo,issue_number),
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE github_deliveries (
    delivery_id BLOB PRIMARY KEY NOT NULL CHECK (typeof(delivery_id)='blob' AND length(delivery_id)=16),
    processed_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE TABLE zotero_connectors (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    owner_user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(owner_user_id)='blob' AND length(owner_user_id)=16),
    library_type TEXT NOT NULL CHECK (library_type IN ('user','group')),
    remote_library_id INTEGER NOT NULL CHECK (remote_library_id>0),
    library_url TEXT NOT NULL CHECK (length(CAST(library_url AS BLOB))<=1024),
    state TEXT NOT NULL DEFAULT 'connected' CHECK (state IN ('connected','disconnected','denied')),
    generation INTEGER NOT NULL DEFAULT 1 CHECK (generation>0),
    completed_version INTEGER NOT NULL DEFAULT 0 CHECK (completed_version>=0),
    progress_version INTEGER CHECK (progress_version>=0),
    committed_pages INTEGER NOT NULL DEFAULT 0 CHECK (committed_pages BETWEEN 0 AND 2147483647),
    retry_at INTEGER,
    reconciliation_required INTEGER NOT NULL DEFAULT 1 CHECK (reconciliation_required IN (0,1)),
    sync_id BLOB CHECK (sync_id IS NULL OR (typeof(sync_id)='blob' AND length(sync_id)=16)),
    sync_expires_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,owner_user_id,library_type,remote_library_id), UNIQUE (workspace_id,owner_user_id,id),
    CHECK ((sync_id IS NULL)=(sync_expires_at IS NULL))
) STRICT;
CREATE TABLE zotero_credentials (
    connector_id BLOB PRIMARY KEY NOT NULL CHECK (typeof(connector_id)='blob' AND length(connector_id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    owner_user_id BLOB NOT NULL CHECK (typeof(owner_user_id)='blob' AND length(owner_user_id)=16),
    sealed_key TEXT NOT NULL CHECK (substr(sealed_key,1,7)='enc:v2:'),
    FOREIGN KEY (workspace_id,owner_user_id,connector_id) REFERENCES zotero_connectors(workspace_id,owner_user_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE zotero_references (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    owner_user_id BLOB NOT NULL CHECK (typeof(owner_user_id)='blob' AND length(owner_user_id)=16),
    connector_id BLOB NOT NULL CHECK (typeof(connector_id)='blob' AND length(connector_id)=16),
    document_id BLOB CHECK (document_id IS NULL OR (typeof(document_id)='blob' AND length(document_id)=16)),
    item_key TEXT NOT NULL CHECK (length(item_key)=8 AND item_key NOT GLOB '*[^23456789ABCDEFGHIJKLMNPQRSTUVWXYZ]*'),
    remote_version INTEGER NOT NULL CHECK (remote_version>=0),
    local_version INTEGER NOT NULL DEFAULT 1 CHECK (local_version>0),
    bibliography TEXT NOT NULL CHECK (json_valid(bibliography) AND json_type(bibliography)='object'),
    return_url TEXT NOT NULL CHECK (length(CAST(return_url AS BLOB))<=2048),
    availability TEXT NOT NULL CHECK (availability IN ('available','trashed','deleted','excluded')),
    UNIQUE (workspace_id,owner_user_id,connector_id,item_key), UNIQUE (workspace_id,owner_user_id,connector_id,id), UNIQUE (workspace_id,document_id),
    FOREIGN KEY (workspace_id,owner_user_id,connector_id) REFERENCES zotero_connectors(workspace_id,owner_user_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id)
) STRICT;
CREATE TABLE zotero_collections (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    owner_user_id BLOB NOT NULL CHECK (typeof(owner_user_id)='blob' AND length(owner_user_id)=16),
    connector_id BLOB NOT NULL CHECK (typeof(connector_id)='blob' AND length(connector_id)=16),
    collection_key TEXT NOT NULL CHECK (length(collection_key)=8 AND collection_key NOT GLOB '*[^23456789ABCDEFGHIJKLMNPQRSTUVWXYZ]*'),
    remote_version INTEGER NOT NULL CHECK (remote_version>=0),
    name TEXT NOT NULL CHECK (length(CAST(name AS BLOB))<=4096),
    parent_key TEXT,
    availability TEXT NOT NULL DEFAULT 'available' CHECK (availability IN ('available','deleted')),
    PRIMARY KEY (workspace_id,owner_user_id,connector_id,collection_key),
    FOREIGN KEY (workspace_id,owner_user_id,connector_id) REFERENCES zotero_connectors(workspace_id,owner_user_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,owner_user_id,connector_id,parent_key) REFERENCES zotero_collections(workspace_id,owner_user_id,connector_id,collection_key) DEFERRABLE INITIALLY DEFERRED,
    CHECK (parent_key IS NULL OR parent_key<>collection_key)
) STRICT;
CREATE TABLE zotero_memberships (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    owner_user_id BLOB NOT NULL CHECK (typeof(owner_user_id)='blob' AND length(owner_user_id)=16),
    connector_id BLOB NOT NULL CHECK (typeof(connector_id)='blob' AND length(connector_id)=16),
    reference_id BLOB NOT NULL CHECK (typeof(reference_id)='blob' AND length(reference_id)=16),
    collection_key TEXT NOT NULL,
    PRIMARY KEY (workspace_id,owner_user_id,connector_id,reference_id,collection_key),
    FOREIGN KEY (workspace_id,owner_user_id,connector_id,reference_id) REFERENCES zotero_references(workspace_id,owner_user_id,connector_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,owner_user_id,connector_id,collection_key) REFERENCES zotero_collections(workspace_id,owner_user_id,connector_id,collection_key) ON DELETE CASCADE
) STRICT;
CREATE TABLE zotero_links (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    owner_user_id BLOB NOT NULL CHECK (typeof(owner_user_id)='blob' AND length(owner_user_id)=16),
    connector_id BLOB NOT NULL CHECK (typeof(connector_id)='blob' AND length(connector_id)=16),
    reference_id BLOB NOT NULL CHECK (typeof(reference_id)='blob' AND length(reference_id)=16),
    document_id BLOB CHECK (document_id IS NULL OR (typeof(document_id)='blob' AND length(document_id)=16)),
    task_id BLOB CHECK (task_id IS NULL OR (typeof(task_id)='blob' AND length(task_id)=16)),
    anchor TEXT NOT NULL DEFAULT '' CHECK (length(anchor)<=256),
    CHECK ((document_id IS NULL)<>(task_id IS NULL)),
    FOREIGN KEY (workspace_id,owner_user_id,connector_id,reference_id) REFERENCES zotero_references(workspace_id,owner_user_id,connector_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE INDEX webhooks_created_by_idx ON webhooks(created_by);
CREATE INDEX webhook_deliveries_pending_next_attempt_at_idx ON webhook_deliveries(next_attempt_at) WHERE status='pending';
CREATE INDEX webhook_deliveries_settled_created_at_idx ON webhook_deliveries(created_at) WHERE status IN ('delivered','failed');
CREATE INDEX github_install_states_workspace_id_idx ON github_install_states(workspace_id);
CREATE INDEX github_install_states_expires_at_idx ON github_install_states(expires_at);
CREATE INDEX github_deliveries_processed_at_idx ON github_deliveries(processed_at);
CREATE UNIQUE INDEX zotero_links_document_unique ON zotero_links(workspace_id,reference_id,document_id,anchor) WHERE document_id IS NOT NULL;
CREATE UNIQUE INDEX zotero_links_task_unique ON zotero_links(workspace_id,reference_id,task_id,anchor) WHERE task_id IS NOT NULL;
CREATE TRIGGER zotero_reference_document_purge BEFORE DELETE ON documents BEGIN
    UPDATE zotero_references SET document_id=NULL WHERE workspace_id=OLD.workspace_id AND document_id=OLD.id;
END;
