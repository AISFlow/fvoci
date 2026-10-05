-- FVOCI SQLite-family new-install baseline, lineage fvoci-sqlite-060, step 11: imports.
-- import jobs, deferred import events and personal-input / personal-transfer command receipts.

CREATE TABLE import_jobs (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    created_by BLOB NOT NULL REFERENCES users(id) CHECK (typeof(created_by)='blob' AND length(created_by)=16),
    session_id BLOB CHECK (session_id IS NULL OR (typeof(session_id)='blob' AND length(session_id)=16)),
    source TEXT NOT NULL CHECK (source IN ('markdown-zip','office-file','notion-zip','native-archive')),
    status TEXT NOT NULL CHECK (status IN ('pending','running','completed','failed')),
    file_name TEXT CHECK (file_name IS NULL OR length(file_name) BETWEEN 1 AND 255),
    project_id BLOB CHECK (project_id IS NULL OR (typeof(project_id)='blob' AND length(project_id)=16)),
    payload BLOB CHECK (payload IS NULL OR length(payload) BETWEEN 1 AND 67108864),
    created_refs TEXT NOT NULL DEFAULT '{"documentIds":[],"taskIds":[],"storedKeys":[]}' CHECK (json_valid(created_refs)
        AND json_type(created_refs,'$.documentIds') IS 'array' AND json_type(created_refs,'$.taskIds') IS 'array' AND json_type(created_refs,'$.storedKeys') IS 'array'),
    lease_token BLOB CHECK (lease_token IS NULL OR (typeof(lease_token)='blob' AND length(lease_token)=16)),
    lease_until INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 2),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    native_request_id BLOB CHECK (native_request_id IS NULL OR (typeof(native_request_id)='blob' AND length(native_request_id)=16)),
    native_archive_hash TEXT,
    native_result TEXT CHECK (native_result IS NULL OR (json_valid(native_result) AND length(CAST(native_result AS BLOB))<=1048576)),
    native_diagnostic TEXT,
    UNIQUE (workspace_id,id), CHECK (status IN ('pending','running') OR payload IS NULL),
    CHECK ((lease_token IS NULL)=(lease_until IS NULL)),
    CHECK ((source='native-archive' AND native_request_id IS NOT NULL AND native_archive_hash IS NOT NULL
        AND length(native_archive_hash)=64 AND native_archive_hash NOT GLOB '*[^0-9a-f]*')
        OR (source<>'native-archive' AND native_request_id IS NULL AND native_archive_hash IS NULL AND native_result IS NULL AND native_diagnostic IS NULL))
) STRICT;
CREATE TABLE import_deferred_events (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    import_job_id BLOB NOT NULL CHECK (typeof(import_job_id)='blob' AND length(import_job_id)=16),
    id BLOB NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    seq INTEGER NOT NULL,
    actor_user_id BLOB CHECK (actor_user_id IS NULL OR (typeof(actor_user_id)='blob' AND length(actor_user_id)=16)),
    verb TEXT NOT NULL,
    target_type TEXT,
    target_id BLOB CHECK (target_id IS NULL OR (typeof(target_id)='blob' AND length(target_id)=16)),
    payload TEXT NOT NULL CHECK (json_valid(payload)),
    channel TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (import_job_id,id),
    FOREIGN KEY (workspace_id,import_job_id) REFERENCES import_jobs(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE personal_input_commands (
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    actor_user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(actor_user_id)='blob' AND length(actor_user_id)=16),
    request_id BLOB NOT NULL CHECK (typeof(request_id)='blob' AND length(request_id)=16),
    request_hash TEXT NOT NULL CHECK (length(request_hash)=64),
    intent TEXT NOT NULL CHECK (intent IN ('quick','note','task')),
    document_id BLOB CHECK (document_id IS NULL OR (typeof(document_id)='blob' AND length(document_id)=16)),
    task_id BLOB CHECK (task_id IS NULL OR (typeof(task_id)='blob' AND length(task_id)=16)),
    project_id BLOB CHECK (project_id IS NULL OR (typeof(project_id)='blob' AND length(project_id)=16)),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (workspace_id,actor_user_id,request_id), CHECK (intent='task' OR (task_id IS NULL AND project_id IS NULL)),
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id),
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id),
    FOREIGN KEY (workspace_id,project_id) REFERENCES projects(workspace_id,id)
) STRICT;
CREATE TABLE personal_transfer_commands (
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    actor_user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(actor_user_id)='blob' AND length(actor_user_id)=16),
    request_id BLOB NOT NULL CHECK (typeof(request_id)='blob' AND length(request_id)=16),
    session_id BLOB NOT NULL CHECK (typeof(session_id)='blob' AND length(session_id)=16),
    request_hash TEXT NOT NULL CHECK (length(request_hash)=64 AND request_hash NOT GLOB '*[^0-9a-f]*'),
    action TEXT NOT NULL CHECK (action IN ('copy','move')),
    destination_workspace_id BLOB NOT NULL CHECK (typeof(destination_workspace_id)='blob' AND length(destination_workspace_id)=16),
    destination_project_id BLOB NOT NULL CHECK (typeof(destination_project_id)='blob' AND length(destination_project_id)=16),
    document_id BLOB NOT NULL CHECK (typeof(document_id)='blob' AND length(document_id)=16),
    document_number INTEGER NOT NULL CHECK (document_number BETWEEN 1 AND 2147483647),
    task_id BLOB CHECK (task_id IS NULL OR (typeof(task_id)='blob' AND length(task_id)=16)),
    task_number INTEGER CHECK (task_number BETWEEN 1 AND 2147483647),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (workspace_id,actor_user_id,request_id),
    CHECK ((task_id IS NULL)=(task_number IS NULL)), CHECK (workspace_id<>destination_workspace_id)
) STRICT;
CREATE INDEX import_jobs_lease_idx ON import_jobs(status,lease_until);
CREATE INDEX import_jobs_workspace_id_idx ON import_jobs(workspace_id);
CREATE UNIQUE INDEX import_jobs_native_request_unique ON import_jobs(workspace_id,created_by,native_request_id) WHERE source='native-archive';
CREATE INDEX import_deferred_events_job_seq_idx ON import_deferred_events(workspace_id,import_job_id,seq);
CREATE TRIGGER personal_input_document_purge BEFORE DELETE ON documents BEGIN
    UPDATE personal_input_commands SET document_id=NULL WHERE workspace_id=OLD.workspace_id AND document_id=OLD.id;
END;
CREATE TRIGGER personal_input_task_purge BEFORE DELETE ON tasks BEGIN
    UPDATE personal_input_commands SET task_id=NULL WHERE workspace_id=OLD.workspace_id AND task_id=OLD.id;
END;
CREATE TRIGGER personal_input_project_purge BEFORE DELETE ON projects BEGIN
    UPDATE personal_input_commands SET project_id=NULL WHERE workspace_id=OLD.workspace_id AND project_id=OLD.id;
END;
