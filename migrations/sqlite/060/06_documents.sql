-- FVOCI SQLite-family new-install baseline, lineage fvoci-sqlite-060, step 06: documents.
-- documents and their rooms, principals, revisions, comments, stars, share links, tags, templates, task origins, OFF command receipts and the room fence lease registry.

CREATE TABLE documents (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    title TEXT NOT NULL,
    icon TEXT,
    path TEXT NOT NULL CHECK (
        length(path) BETWEEN 32 AND 659 AND path NOT GLOB '*[^0-9a-f.]*'
        AND (length(path)+1)%33=0 AND path NOT GLOB '.*' AND path NOT GLOB '*.'
        AND replace(path,'.','') NOT GLOB '*[^0-9a-f]*'
    ),
    parent_id BLOB CHECK (parent_id IS NULL OR (typeof(parent_id)='blob' AND length(parent_id)=16)),
    sort_key TEXT NOT NULL,
    project_id BLOB CHECK (project_id IS NULL OR (typeof(project_id)='blob' AND length(project_id)=16)),
    number INTEGER NOT NULL CHECK (number BETWEEN -2147483648 AND 2147483647),
    status TEXT NOT NULL CHECK (status IN ('draft','published','archived')),
    schema_version INTEGER NOT NULL CHECK (schema_version BETWEEN -2147483648 AND 2147483647),
    text TEXT NOT NULL DEFAULT '',
    chosung TEXT NOT NULL DEFAULT '',
    version INTEGER NOT NULL DEFAULT 1 CHECK (version BETWEEN -2147483648 AND 2147483647),
    created_by BLOB NOT NULL REFERENCES users(id) CHECK (typeof(created_by)='blob' AND length(created_by)=16),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    deleted_at INTEGER,
    content_json TEXT NOT NULL CHECK (json_valid(content_json)),
    kind TEXT NOT NULL DEFAULT 'doc' CHECK (kind IN ('doc','wiki','template')),
    UNIQUE (workspace_id,id),
    FOREIGN KEY (workspace_id,parent_id) REFERENCES documents(workspace_id,id),
    FOREIGN KEY (workspace_id,project_id) REFERENCES projects(workspace_id,id)
) STRICT;
CREATE TABLE document_states (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    document_id BLOB NOT NULL CHECK (typeof(document_id)='blob' AND length(document_id)=16),
    state BLOB NOT NULL,
    encoding INTEGER NOT NULL DEFAULT 1 CHECK (encoding=1),
    compacted_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    writer_generation INTEGER NOT NULL DEFAULT 0,
    snapshot_cutoff_seq INTEGER NOT NULL DEFAULT 0,
    tail_seq INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (workspace_id,document_id),
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE document_collab_updates (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    document_id BLOB NOT NULL CHECK (typeof(document_id)='blob' AND length(document_id)=16),
    seq INTEGER NOT NULL,
    op_id BLOB NOT NULL CHECK (typeof(op_id)='blob' AND length(op_id)=16),
    payload BLOB NOT NULL CHECK (length(payload) BETWEEN 1 AND 8388608),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (workspace_id,document_id,seq),
    UNIQUE (workspace_id,document_id,op_id),
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE document_collab_op_receipts (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    document_id BLOB NOT NULL CHECK (typeof(document_id)='blob' AND length(document_id)=16),
    op_id BLOB NOT NULL CHECK (typeof(op_id)='blob' AND length(op_id)=16),
    seq INTEGER NOT NULL,
    payload_len INTEGER NOT NULL CHECK (payload_len BETWEEN 1 AND 8388608),
    payload_sha256 BLOB NOT NULL CHECK (length(payload_sha256)=32),
    actor_user_id BLOB NOT NULL REFERENCES users(id) CHECK (typeof(actor_user_id)='blob' AND length(actor_user_id)=16),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (workspace_id,document_id,op_id),
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE revisions (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    target_kind TEXT NOT NULL CHECK (target_kind IN ('document','task')),
    target_id BLOB NOT NULL CHECK (typeof(target_id)='blob' AND length(target_id)=16),
    y_snapshot BLOB NOT NULL,
    encoding INTEGER NOT NULL DEFAULT 1 CHECK (encoding=1),
    content_json TEXT NOT NULL CHECK (json_valid(content_json)),
    text TEXT NOT NULL,
    reason TEXT NOT NULL CHECK (reason IN ('manual','session','scheduled','restore')),
    created_by BLOB REFERENCES users(id) CHECK (created_by IS NULL OR (typeof(created_by)='blob' AND length(created_by)=16)),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    restored_from_id BLOB CHECK (restored_from_id IS NULL OR (typeof(restored_from_id)='blob' AND length(restored_from_id)=16)),
    restore_correlation_id BLOB CHECK (restore_correlation_id IS NULL OR (typeof(restore_correlation_id)='blob' AND length(restore_correlation_id)=16)),
    restore_base_tail_seq INTEGER,
    restore_committed_tail_seq INTEGER,
    UNIQUE (workspace_id,id),
    CHECK ((reason='restore' AND created_by IS NOT NULL AND restored_from_id IS NOT NULL
        AND restore_correlation_id IS NOT NULL AND restore_base_tail_seq IS NOT NULL AND restore_base_tail_seq>=0
        AND restore_committed_tail_seq IS NOT NULL AND restore_committed_tail_seq>restore_base_tail_seq
        AND restore_committed_tail_seq-restore_base_tail_seq=1)
        OR (reason<>'restore' AND restored_from_id IS NULL AND restore_correlation_id IS NULL
        AND restore_base_tail_seq IS NULL AND restore_committed_tail_seq IS NULL))
) STRICT;
CREATE TABLE document_members (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    document_id BLOB NOT NULL CHECK (typeof(document_id)='blob' AND length(document_id)=16),
    user_id BLOB CHECK (user_id IS NULL OR (typeof(user_id)='blob' AND length(user_id)=16)),
    group_id BLOB CHECK (group_id IS NULL OR (typeof(group_id)='blob' AND length(group_id)=16)),
    role TEXT NOT NULL CHECK (role IN ('lead','member','viewer')),
    UNIQUE (workspace_id,id), CHECK ((user_id IS NULL)<>(group_id IS NULL)),
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,group_id) REFERENCES groups(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE comments (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    document_id BLOB CHECK (document_id IS NULL OR (typeof(document_id)='blob' AND length(document_id)=16)),
    task_id BLOB CHECK (task_id IS NULL OR (typeof(task_id)='blob' AND length(task_id)=16)),
    parent_id BLOB CHECK (parent_id IS NULL OR (typeof(parent_id)='blob' AND length(parent_id)=16)),
    created_by BLOB NOT NULL REFERENCES users(id) CHECK (typeof(created_by)='blob' AND length(created_by)=16),
    body TEXT NOT NULL CHECK (length(trim(body))>=1 AND length(body)<=8000),
    chosung TEXT NOT NULL DEFAULT '',
    resolved_at INTEGER,
    reactions TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(reactions)),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id), CHECK ((document_id IS NULL)<>(task_id IS NULL)),
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,parent_id) REFERENCES comments(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE stars (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    document_id BLOB CHECK (document_id IS NULL OR (typeof(document_id)='blob' AND length(document_id)=16)),
    task_id BLOB CHECK (task_id IS NULL OR (typeof(task_id)='blob' AND length(task_id)=16)),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id), CHECK ((document_id IS NULL)<>(task_id IS NULL)),
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE share_links (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    token_hash TEXT NOT NULL UNIQUE CHECK (length(token_hash)=64 AND token_hash NOT GLOB '*[^0-9a-f]*'),
    document_id BLOB CHECK (document_id IS NULL OR (typeof(document_id)='blob' AND length(document_id)=16)),
    project_id BLOB CHECK (project_id IS NULL OR (typeof(project_id)='blob' AND length(project_id)=16)),
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id), CHECK ((document_id IS NULL)<>(project_id IS NULL)),
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,project_id) REFERENCES projects(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE document_tags (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    name TEXT NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 100),
    color TEXT NOT NULL CHECK (color IN ('gray','red','orange','amber','green','teal','blue','violet','pink')),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id)
) STRICT;
CREATE TABLE document_tag_assignments (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    document_id BLOB NOT NULL CHECK (typeof(document_id)='blob' AND length(document_id)=16),
    tag_id BLOB NOT NULL CHECK (typeof(tag_id)='blob' AND length(tag_id)=16),
    PRIMARY KEY (workspace_id,document_id,tag_id),
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,tag_id) REFERENCES document_tags(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE task_origins (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    document_id BLOB NOT NULL CHECK (typeof(document_id)='blob' AND length(document_id)=16),
    request_id BLOB NOT NULL CHECK (typeof(request_id)='blob' AND length(request_id)=16),
    request_hash TEXT NOT NULL,
    anchor TEXT CHECK (anchor IS NULL OR length(anchor)<=200),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (workspace_id,task_id), UNIQUE (workspace_id,document_id,request_id),
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE templates (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    kind TEXT NOT NULL CHECK (kind IN ('document','task')),
    title TEXT NOT NULL CHECK (length(trim(title)) BETWEEN 1 AND 200),
    payload TEXT NOT NULL CHECK (json_valid(payload)),
    created_by BLOB NOT NULL REFERENCES users(id) CHECK (typeof(created_by)='blob' AND length(created_by)=16),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id)
) STRICT;
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
CREATE TABLE collab_fence_counter (
    id INT PRIMARY KEY NOT NULL CHECK (id=1),
    next_fence INTEGER NOT NULL CHECK (typeof(next_fence)='integer' AND next_fence>0)
) STRICT;
CREATE TABLE collab_room_fences (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    document_id BLOB NOT NULL CHECK (typeof(document_id)='blob' AND length(document_id)=16),
    owner_token BLOB NOT NULL CHECK (typeof(owner_token)='blob' AND length(owner_token)=16),
    fence INTEGER NOT NULL CHECK (typeof(fence)='integer' AND fence>0),
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (workspace_id,document_id),
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE CASCADE
) STRICT;
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
CREATE TABLE task_collab_room_fences (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    owner_token BLOB NOT NULL CHECK (typeof(owner_token)='blob' AND length(owner_token)=16),
    fence INTEGER NOT NULL CHECK (typeof(fence)='integer' AND fence>0),
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (workspace_id,task_id),
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE UNIQUE INDEX documents_workspace_project_number_unique ON documents(workspace_id,project_id,number) WHERE project_id IS NOT NULL;
CREATE UNIQUE INDEX documents_workspace_wiki_number_unique ON documents(workspace_id,number) WHERE project_id IS NULL;
CREATE INDEX documents_created_by_idx ON documents(created_by);
CREATE INDEX documents_workspace_id_parent_id_idx ON documents(workspace_id,parent_id);
CREATE INDEX documents_workspace_id_project_id_idx ON documents(workspace_id,project_id);
CREATE INDEX documents_workspace_id_updated_at_id_idx ON documents(workspace_id,updated_at,id) WHERE deleted_at IS NULL;
CREATE INDEX documents_workspace_id_updated_at_live_idx ON documents(workspace_id,updated_at DESC,id DESC) WHERE deleted_at IS NULL;
CREATE INDEX document_collab_updates_tail_idx ON document_collab_updates(workspace_id,document_id,seq);
CREATE INDEX document_collab_op_receipts_lookup_idx ON document_collab_op_receipts(workspace_id,document_id,seq);
CREATE INDEX revisions_workspace_id_target_id_created_at_id_idx ON revisions(workspace_id,target_id,created_at DESC,id DESC);
CREATE UNIQUE INDEX revisions_workspace_restore_correlation_idx ON revisions(workspace_id,restore_correlation_id) WHERE restore_correlation_id IS NOT NULL;
CREATE UNIQUE INDEX document_members_user_unique ON document_members(workspace_id,document_id,user_id);
CREATE UNIQUE INDEX document_members_group_unique ON document_members(workspace_id,document_id,group_id);
CREATE INDEX document_members_workspace_id_user_id_idx ON document_members(workspace_id,user_id);
CREATE INDEX document_members_workspace_id_group_id_idx ON document_members(workspace_id,group_id) WHERE group_id IS NOT NULL;
CREATE INDEX comments_workspace_id_parent_id_idx ON comments(workspace_id,parent_id);
CREATE INDEX comments_workspace_id_created_by_idx ON comments(workspace_id,created_by,created_at,id);
CREATE INDEX comments_workspace_id_document_id_created_at_idx ON comments(workspace_id,document_id,created_at,id);
CREATE INDEX comments_workspace_id_task_id_created_at_idx ON comments(workspace_id,task_id,created_at,id);
CREATE UNIQUE INDEX stars_user_document_unique ON stars(workspace_id,user_id,document_id);
CREATE UNIQUE INDEX stars_user_task_unique ON stars(workspace_id,user_id,task_id);
CREATE INDEX stars_workspace_id_document_id_idx ON stars(workspace_id,document_id);
CREATE INDEX stars_workspace_id_task_id_idx ON stars(workspace_id,task_id);
CREATE INDEX share_links_workspace_id_document_id_idx ON share_links(workspace_id,document_id);
CREATE INDEX share_links_workspace_id_project_id_idx ON share_links(workspace_id,project_id);
CREATE INDEX share_links_workspace_id_user_id_idx ON share_links(workspace_id,user_id);
CREATE UNIQUE INDEX document_tags_workspace_id_lower_name_idx ON document_tags(workspace_id,lower(name));
CREATE INDEX document_tag_assignments_workspace_id_tag_id_idx ON document_tag_assignments(workspace_id,tag_id);
CREATE INDEX task_origins_document_task_idx ON task_origins(workspace_id,document_id,task_id);
CREATE INDEX templates_workspace_id_idx ON templates(workspace_id);
CREATE INDEX templates_created_by_idx ON templates(created_by);
INSERT INTO collab_fence_counter(id,next_fence) VALUES (1,1);
CREATE TRIGGER documents_path_insert BEFORE INSERT ON documents BEGIN
    SELECT RAISE(ABORT,'documents path segment') WHERE EXISTS (
        WITH RECURSIVE segments(rest,segment) AS (
            SELECT NEW.path||'.',''
            UNION ALL SELECT substr(rest,instr(rest,'.')+1),substr(rest,1,instr(rest,'.')-1) FROM segments WHERE rest<>''
        ) SELECT 1 FROM segments WHERE rest<>NEW.path||'.' AND length(segment)<>32
    );
END;
CREATE TRIGGER documents_path_update BEFORE UPDATE OF path ON documents BEGIN
    SELECT RAISE(ABORT,'documents path segment') WHERE EXISTS (
        WITH RECURSIVE segments(rest,segment) AS (
            SELECT NEW.path||'.',''
            UNION ALL SELECT substr(rest,instr(rest,'.')+1),substr(rest,1,instr(rest,'.')-1) FROM segments WHERE rest<>''
        ) SELECT 1 FROM segments WHERE rest<>NEW.path||'.' AND length(segment)<>32
    );
END;
CREATE TRIGGER wiki_create_commands_document_purge BEFORE DELETE ON documents BEGIN
    UPDATE wiki_create_commands SET document_id=NULL
    WHERE workspace_id=OLD.workspace_id AND document_id=OLD.id;
END;
CREATE TRIGGER collab_fence_counter_no_delete BEFORE DELETE ON collab_fence_counter BEGIN
    SELECT RAISE(ABORT,'collab fence counter cannot be deleted');
END;
CREATE TRIGGER collab_fence_counter_no_reset BEFORE UPDATE ON collab_fence_counter
WHEN NEW.id<>OLD.id OR NEW.next_fence<OLD.next_fence BEGIN
    SELECT RAISE(ABORT,'collab fence counter cannot be reset');
END;
CREATE TRIGGER collab_fence_counter_no_replace BEFORE INSERT ON collab_fence_counter
WHEN EXISTS (SELECT 1 FROM collab_fence_counter) BEGIN
    SELECT RAISE(ABORT,'collab fence counter already exists');
END;
CREATE TRIGGER body_save_commands_document_purge BEFORE DELETE ON documents BEGIN
    UPDATE body_save_commands SET document_id=NULL WHERE workspace_id=OLD.workspace_id AND document_id=OLD.id;
END;
CREATE TRIGGER body_save_commands_task_purge BEFORE DELETE ON tasks BEGIN
    UPDATE body_save_commands SET task_id=NULL WHERE workspace_id=OLD.workspace_id AND task_id=OLD.id;
END;
