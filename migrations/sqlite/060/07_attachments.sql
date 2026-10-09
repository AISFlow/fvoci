-- FVOCI SQLite-family new-install baseline, lineage fvoci-sqlite-060, step 07: attachments.
-- attachments, extracted text and the storage-key cleanup journal.

CREATE TABLE attachments (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    document_id BLOB CHECK (document_id IS NULL OR (typeof(document_id)='blob' AND length(document_id)=16)),
    task_id BLOB CHECK (task_id IS NULL OR (typeof(task_id)='blob' AND length(task_id)=16)),
    uploader_id BLOB NOT NULL REFERENCES users(id) CHECK (typeof(uploader_id)='blob' AND length(uploader_id)=16),
    status TEXT NOT NULL CHECK (status IN ('uploading','assembling','stored')),
    name TEXT NOT NULL,
    mime TEXT NOT NULL DEFAULT 'application/octet-stream',
    declared_mime TEXT,
    size_bytes INTEGER,
    reserved_size_bytes INTEGER NOT NULL CHECK (reserved_size_bytes>0 AND reserved_size_bytes<=9007199254740991),
    storage_key TEXT NOT NULL UNIQUE,
    image INTEGER NOT NULL DEFAULT 0 CHECK (image IN (0,1)),
    variants TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(variants)),
    extract_text TEXT NOT NULL DEFAULT '',
    extract_status TEXT NOT NULL DEFAULT 'skipped' CHECK (extract_status IN ('pending','skipped','ok','empty','partial','unsupported','corrupt','resource_limit','worker_failure')),
    scan_status TEXT NOT NULL DEFAULT 'skipped' CHECK (scan_status IN ('skipped','clean','infected')),
    upload_meta TEXT CHECK (upload_meta IS NULL OR json_valid(upload_meta)),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    completed_at INTEGER,
    extract_lease_token BLOB CHECK (extract_lease_token IS NULL OR (typeof(extract_lease_token)='blob' AND length(extract_lease_token)=16)),
    extract_lease_expires_at INTEGER,
    extract_attempts INTEGER NOT NULL DEFAULT 0 CHECK (extract_attempts BETWEEN 0 AND 2),
    extract_warnings TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(extract_warnings) AND json_type(extract_warnings)='array' AND json_array_length(extract_warnings)<=32),
    extract_rhwp_rev TEXT,
    preview_status TEXT NOT NULL DEFAULT 'skipped' CHECK (preview_status IN ('pending','skipped','ok','failed')),
    preview_lease_token BLOB CHECK (preview_lease_token IS NULL OR (typeof(preview_lease_token)='blob' AND length(preview_lease_token)=16)),
    preview_lease_expires_at INTEGER,
    preview_attempts INTEGER NOT NULL DEFAULT 0 CHECK (preview_attempts BETWEEN 0 AND 3),
    UNIQUE (workspace_id,id), CHECK ((document_id IS NULL)<>(task_id IS NULL)),
    CHECK ((status='stored' AND size_bytes IS NOT NULL AND size_bytes=reserved_size_bytes AND completed_at IS NOT NULL)
        OR (status<>'stored' AND size_bytes IS NULL AND completed_at IS NULL)),
    CHECK ((extract_lease_token IS NULL)=(extract_lease_expires_at IS NULL)),
    CHECK ((preview_lease_token IS NULL)=(preview_lease_expires_at IS NULL)),
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id),
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id)
) STRICT;
CREATE TABLE attachment_text (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    attachment_id BLOB NOT NULL CHECK (typeof(attachment_id)='blob' AND length(attachment_id)=16),
    chunk_no INTEGER NOT NULL CHECK (chunk_no BETWEEN 0 AND 2147483647),
    start_offset INTEGER NOT NULL CHECK (start_offset BETWEEN 0 AND 2147483647),
    end_offset INTEGER NOT NULL CHECK (end_offset BETWEEN start_offset AND 2147483647),
    text TEXT NOT NULL,
    chosung TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL CHECK (status IN ('ok','skipped','empty','partial','unsupported','corrupt','resource_limit','worker_failure')),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    embedding TEXT CHECK (embedding IS NULL OR (json_valid(embedding) AND json_type(embedding)='array' AND json_array_length(embedding)=1536)),
    PRIMARY KEY (workspace_id,attachment_id,chunk_no),
    FOREIGN KEY (workspace_id,attachment_id) REFERENCES attachments(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE attachment_object_cleanups (
    id BLOB PRIMARY KEY NOT NULL DEFAULT (randomblob(16)) CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    attachment_id BLOB NOT NULL CHECK (typeof(attachment_id)='blob' AND length(attachment_id)=16),
    storage_key TEXT NOT NULL,
    due_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 2147483647)
) STRICT;
CREATE INDEX attachments_workspace_id_document_id_idx ON attachments(workspace_id,document_id);
CREATE INDEX attachments_workspace_id_task_id_idx ON attachments(workspace_id,task_id);
CREATE INDEX attachments_uploader_id_idx ON attachments(uploader_id);
CREATE INDEX attachments_uploading_created_at_idx ON attachments(status,created_at) WHERE status IN ('uploading','assembling');
CREATE INDEX attachments_uploader_id_stored_idx ON attachments(uploader_id,created_at,id) WHERE status='stored';
CREATE INDEX attachments_extract_pending_idx ON attachments(completed_at,id) WHERE status='stored' AND extract_status='pending';
CREATE INDEX attachments_preview_pending_idx ON attachments(completed_at,id) WHERE status='stored' AND preview_status='pending';
CREATE INDEX attachment_text_workspace_attachment_idx ON attachment_text(workspace_id,attachment_id);
CREATE INDEX attachment_text_pending_embedding_idx ON attachment_text(workspace_id,attachment_id) WHERE embedding IS NULL AND text<>'';
CREATE INDEX attachment_object_cleanups_due_idx ON attachment_object_cleanups(due_at,id);
CREATE TRIGGER attachment_object_cleanups_after_attachment_delete AFTER DELETE ON attachments BEGIN
    INSERT INTO attachment_object_cleanups(id,workspace_id,attachment_id,storage_key)
        VALUES (randomblob(16),OLD.workspace_id,OLD.id,OLD.storage_key);
    INSERT INTO attachment_object_cleanups(id,workspace_id,attachment_id,storage_key)
        SELECT randomblob(16),OLD.workspace_id,OLD.id,json_extract(OLD.variants,'$.preview.key')
        WHERE json_type(OLD.variants,'$.preview.key')='text';
END;
