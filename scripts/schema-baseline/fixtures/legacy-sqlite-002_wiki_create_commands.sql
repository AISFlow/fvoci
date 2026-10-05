-- SQLite-family step 2, corresponding to fixed PG055 at
-- b53fd05c3b018927cf50474eaed41441cdd89ed3 (SQL SHA256
-- dc60f939985a5c000d57a7f266c12c247373601f92759630b324b22b4a29af45).
-- Current auth/ACL, active/trashed target checks, original-result replay and
-- retired-command refusal are named Rust operation policy, not DDL authorization.
CREATE TABLE wiki_create_commands (
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE
        CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    command_id BLOB NOT NULL CHECK (typeof(command_id)='blob' AND length(command_id)=16),
    actor_user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE
        CHECK (typeof(actor_user_id)='blob' AND length(actor_user_id)=16),
    request_hash TEXT NOT NULL CHECK (length(request_hash)=64 AND request_hash NOT GLOB '*[^0-9a-f]*'),
    document_id BLOB CHECK (document_id IS NULL OR (typeof(document_id)='blob' AND length(document_id)=16)),
    result_json TEXT NOT NULL CHECK (json_valid(result_json) AND json_type(result_json)='object'),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (workspace_id,command_id),
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE NO ACTION
) STRICT;
-- SQLite composite SET NULL would clear workspace_id too. Clear only the live
-- document binding before FK enforcement, preserving command/actor/hash/result.
CREATE TRIGGER wiki_create_commands_document_purge BEFORE DELETE ON documents BEGIN
    UPDATE wiki_create_commands SET document_id=NULL
    WHERE workspace_id=OLD.workspace_id AND document_id=OLD.id;
END;
