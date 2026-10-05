-- Current SQLite-family OFF command lineage. Credential is a session or PAT;
-- current credential/target authorization remains the named transaction policy.
CREATE TABLE body_save_commands (
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (length(workspace_id)=16),
    command_id BLOB NOT NULL CHECK (length(command_id)=16),
    actor_user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (length(actor_user_id)=16),
    credential_id BLOB NOT NULL CHECK (length(credential_id)=16),
    target_kind TEXT NOT NULL CHECK (target_kind IN ('document','task')),
    target_id BLOB NOT NULL CHECK (length(target_id)=16),
    document_id BLOB CHECK (document_id IS NULL OR length(document_id)=16),
    task_id BLOB CHECK (task_id IS NULL OR length(task_id)=16),
    expected_tail_seq INTEGER NOT NULL CHECK (expected_tail_seq>=0 AND expected_tail_seq<9223372036854775807),
    committed_tail_seq INTEGER NOT NULL CHECK (committed_tail_seq=expected_tail_seq+1),
    request_hash TEXT NOT NULL CHECK (length(request_hash)=64 AND request_hash NOT GLOB '*[^0-9a-f]*'),
    payload_hash BLOB NOT NULL CHECK (length(payload_hash)=32),
    result_json TEXT NOT NULL CHECK (json_valid(result_json) AND json_type(result_json)='object'),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (workspace_id,command_id),
    CHECK ((target_kind='document' AND task_id IS NULL AND (document_id IS NULL OR document_id=target_id))
        OR (target_kind='task' AND document_id IS NULL AND (task_id IS NULL OR task_id=target_id))),
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE NO ACTION,
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE NO ACTION
) STRICT;
CREATE TRIGGER body_save_commands_document_purge BEFORE DELETE ON documents BEGIN
    UPDATE body_save_commands SET document_id=NULL WHERE workspace_id=OLD.workspace_id AND document_id=OLD.id;
END;
CREATE TRIGGER body_save_commands_task_purge BEFORE DELETE ON tasks BEGIN
    UPDATE body_save_commands SET task_id=NULL WHERE workspace_id=OLD.workspace_id AND task_id=OLD.id;
END;
