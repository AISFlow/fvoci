-- SQLite-family baseline of the current PG001..054 data model.
-- This is its own lineage, not 54 synthetic PostgreSQL migration receipts.
-- The runner owns BEGIN IMMEDIATE, FK=ON, WAL/FULL, exact SQL digests and
-- the applied-step marker in the SAME transaction as this DDL.
-- STRICT enforces storage classes; UUIDs are exact 16-byte BLOBs. UTC instants
-- are signed epoch microseconds, decoded/encoded by checked Rust codecs.
-- Defaults use SQLite's millisecond clock without a floating-point conversion;
-- an explicitly bound microsecond timestamp retains all six fractional digits.
-- TEXT uses BINARY ordering. Callers specify NULL rank and timestamp/ID ties.
-- Decimal TEXT has no SQLite numeric comparison/sum contract: named consumers
-- must use exact decimal policy, never REAL/CAST-to-REAL or lexical numeric sort.
-- RLS/definers, current credentials/ACL, deferred private-lead validation and
-- import event routing belong to named Rust operations; a DB operator is trusted.

-- PG001,003,025: identity/workspace/session and personal workspace binding.
CREATE TABLE users (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id) = 'blob' AND length(id) = 16),
    email TEXT NOT NULL UNIQUE CHECK (length(email) > 0 AND email NOT GLOB '*[^!-~]*' AND email = lower(email)),
    password_hash TEXT,
    given_name TEXT NOT NULL,
    family_name TEXT,
    text_scale INTEGER NOT NULL DEFAULT 16 CHECK (text_scale IN (16,18,20)),
    locale TEXT NOT NULL DEFAULT 'ko',
    timezone TEXT NOT NULL DEFAULT 'Asia/Seoul',
    week_starts_on INTEGER NOT NULL DEFAULT 1 CHECK (week_starts_on IN (0,1)),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    deleted_at INTEGER,
    email_verified_at INTEGER,
    anonymized_at INTEGER,
    suspended_at INTEGER,
    is_instance_admin INTEGER NOT NULL DEFAULT 0 CHECK (is_instance_admin IN (0,1)),
    auth_generation INTEGER NOT NULL DEFAULT 0 CHECK (auth_generation BETWEEN -2147483648 AND 2147483647),
    personal_workspace_id BLOB CHECK (personal_workspace_id IS NULL OR (typeof(personal_workspace_id)='blob' AND length(personal_workspace_id)=16)),
    withdraw_cancel_token_hash TEXT,
    FOREIGN KEY (personal_workspace_id) REFERENCES workspaces(id) ON DELETE SET NULL
) STRICT;
CREATE UNIQUE INDEX users_personal_workspace_id_unique ON users(personal_workspace_id) WHERE personal_workspace_id IS NOT NULL;
CREATE UNIQUE INDEX users_withdraw_cancel_token_hash_unique ON users(withdraw_cancel_token_hash) WHERE withdraw_cancel_token_hash IS NOT NULL;
CREATE INDEX users_withdrawn_due_idx ON users(deleted_at) WHERE deleted_at IS NOT NULL AND anonymized_at IS NULL;

CREATE TABLE workspaces (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    slug TEXT NOT NULL UNIQUE CHECK (length(slug) BETWEEN 2 AND 32 AND slug NOT GLOB '*[^a-z0-9-]*'),
    name TEXT NOT NULL,
    settings TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(settings)),
    kind TEXT NOT NULL DEFAULT 'team' CHECK (kind IN ('team','personal')),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    deleted_at INTEGER,
    next_document_number INTEGER NOT NULL DEFAULT 0 CHECK (next_document_number BETWEEN 0 AND 2147483647),
    auto_join_domains TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(auto_join_domains) AND json_type(auto_join_domains)='array')
) STRICT;

CREATE TABLE memberships (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    role TEXT NOT NULL CHECK (role IN ('owner','admin','member','guest')),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (workspace_id,user_id),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(id),
    FOREIGN KEY (user_id) REFERENCES users(id)
) STRICT;

CREATE TABLE sessions (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    user_id BLOB NOT NULL REFERENCES users(id) CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    token_hash TEXT NOT NULL UNIQUE,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;

-- PG001,013,043: seq is allocated in the serialized writer transaction.
-- PostgreSQL-only xact is omitted; family visibility and cursors use seq.
CREATE TABLE events (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    seq INTEGER NOT NULL UNIQUE CHECK (typeof(seq)='integer' AND seq>0),
    workspace_id BLOB CHECK (workspace_id IS NULL OR (typeof(workspace_id)='blob' AND length(workspace_id)=16)),
    actor_user_id BLOB CHECK (actor_user_id IS NULL OR (typeof(actor_user_id)='blob' AND length(actor_user_id)=16)),
    verb TEXT NOT NULL,
    target_type TEXT,
    target_id BLOB CHECK (target_id IS NULL OR (typeof(target_id)='blob' AND length(target_id)=16)),
    payload TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(payload)),
    channel TEXT NOT NULL DEFAULT 'web' CHECK (channel IN ('web','api','mcp','webhook','system')),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE INDEX events_relay_idx ON events(seq);
CREATE INDEX events_workspace_idx ON events(workspace_id,created_at);
CREATE INDEX events_workspace_relay_idx ON events(workspace_id,seq);

CREATE TABLE audit_log (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    actor_user_id BLOB CHECK (actor_user_id IS NULL OR (typeof(actor_user_id)='blob' AND length(actor_user_id)=16)),
    workspace_id BLOB CHECK (workspace_id IS NULL OR (typeof(workspace_id)='blob' AND length(workspace_id)=16)),
    verb TEXT NOT NULL,
    target_type TEXT,
    target_id BLOB CHECK (target_id IS NULL OR (typeof(target_id)='blob' AND length(target_id)=16)),
    payload TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(payload)),
    ip TEXT, -- checked inet codec in the consumer; no SQLite IP parser
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE INDEX audit_log_created_at_id_idx ON audit_log(created_at DESC,id DESC);

-- PG004,008: unqualified current names, scoped parents and project numbers.
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
-- PG NULLS NOT DISTINCT: nullable project and non-null project are disjoint.
CREATE UNIQUE INDEX documents_workspace_project_number_unique ON documents(workspace_id,project_id,number) WHERE project_id IS NOT NULL;
CREATE UNIQUE INDEX documents_workspace_wiki_number_unique ON documents(workspace_id,number) WHERE project_id IS NULL;
CREATE INDEX documents_created_by_idx ON documents(created_by);
CREATE INDEX documents_workspace_id_parent_id_idx ON documents(workspace_id,parent_id);
CREATE INDEX documents_workspace_id_project_id_idx ON documents(workspace_id,project_id);
CREATE INDEX documents_workspace_id_updated_at_id_idx ON documents(workspace_id,updated_at,id) WHERE deleted_at IS NULL;
CREATE INDEX documents_workspace_id_updated_at_live_idx ON documents(workspace_id,updated_at DESC,id DESC) WHERE deleted_at IS NULL;
-- Check every path segment (SQLite CHECK cannot contain a subquery).
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
CREATE INDEX document_collab_updates_tail_idx ON document_collab_updates(workspace_id,document_id,seq);
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
CREATE INDEX document_collab_op_receipts_lookup_idx ON document_collab_op_receipts(workspace_id,document_id,seq);

-- PG010,037,050: polymorphic target/provenance are historical locators, not FKs.
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
CREATE INDEX revisions_workspace_id_target_id_created_at_id_idx ON revisions(workspace_id,target_id,created_at DESC,id DESC);
CREATE UNIQUE INDEX revisions_workspace_restore_correlation_idx ON revisions(workspace_id,restore_correlation_id) WHERE restore_correlation_id IS NOT NULL;

-- PG012: scopes are a JSON array of the same closed strings as text[].
CREATE TABLE api_tokens (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    user_id BLOB CHECK (user_id IS NULL OR (typeof(user_id)='blob' AND length(user_id)=16)),
    token_hash TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    scopes TEXT NOT NULL CHECK (json_valid(scopes) AND json_type(scopes)='array' AND json_array_length(scopes)>0),
    expires_at INTEGER,
    last_used_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id),
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX api_tokens_workspace_id_user_id_idx ON api_tokens(workspace_id,user_id);
CREATE TRIGGER api_tokens_scopes_insert BEFORE INSERT ON api_tokens BEGIN
    SELECT RAISE(ABORT,'api token scopes') WHERE EXISTS (SELECT 1 FROM json_each(NEW.scopes)
        WHERE type<>'text' OR value NOT IN ('documents.read','documents.write','tasks.read','tasks.write','projects.read','projects.manage','share.manage','workspace.manage'));
END;
CREATE TRIGGER api_tokens_scopes_update BEFORE UPDATE OF scopes ON api_tokens BEGIN
    SELECT RAISE(ABORT,'api token scopes') WHERE EXISTS (SELECT 1 FROM json_each(NEW.scopes)
        WHERE type<>'text' OR value NOT IN ('documents.read','documents.write','tasks.read','tasks.write','projects.read','projects.manage','share.manage','workspace.manage'));
END;

-- PG008,015..017,037,049: current projects/tasks, principals and exact estimates.
CREATE TABLE projects (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    key TEXT NOT NULL CHECK (length(key) BETWEEN 2 AND 32 AND key GLOB '[A-Z]*' AND key NOT GLOB '*[^A-Z0-9-]*'
        AND (key NOT GLOB '*[0-9]' OR substr(rtrim(key,'0123456789'),-1)<>'-') AND key NOT IN ('WIKI','PROJECTS','SEARCH','MY-TASKS','TRASH','NOTIFICATIONS','SETTINGS','A')),
    name TEXT NOT NULL CHECK (length(trim(name))>=1 AND length(name)<=200),
    description TEXT,
    icon TEXT,
    visibility TEXT NOT NULL CHECK (visibility IN ('private','workspace')),
    root_document_id BLOB CHECK (root_document_id IS NULL OR (typeof(root_document_id)='blob' AND length(root_document_id)=16)),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived')),
    next_number INTEGER NOT NULL DEFAULT 1 CHECK (next_number BETWEEN 1 AND 2147483647),
    created_by BLOB NOT NULL REFERENCES users(id) CHECK (typeof(created_by)='blob' AND length(created_by)=16),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    deleted_at INTEGER,
    UNIQUE (workspace_id,id), UNIQUE (workspace_id,key)
) STRICT;
CREATE INDEX projects_workspace_id_deleted_at_idx ON projects(workspace_id,deleted_at);

CREATE TABLE workflows (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    project_id BLOB NOT NULL CHECK (typeof(project_id)='blob' AND length(project_id)=16),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id),
    FOREIGN KEY (workspace_id,project_id) REFERENCES projects(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE statuses (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    project_id BLOB NOT NULL CHECK (typeof(project_id)='blob' AND length(project_id)=16),
    workflow_id BLOB NOT NULL CHECK (typeof(workflow_id)='blob' AND length(workflow_id)=16),
    name TEXT NOT NULL,
    category TEXT NOT NULL CHECK (category IN ('backlog','todo','in_progress','done','canceled')),
    sort_key TEXT NOT NULL,
    wip_limit INTEGER CHECK (wip_limit BETWEEN -2147483648 AND 2147483647),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id), UNIQUE (workspace_id,project_id,id),
    FOREIGN KEY (workspace_id,project_id) REFERENCES projects(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,workflow_id) REFERENCES workflows(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE INDEX statuses_workspace_project_idx ON statuses(workspace_id,project_id,sort_key COLLATE BINARY);
CREATE TABLE tasks (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    project_id BLOB NOT NULL CHECK (typeof(project_id)='blob' AND length(project_id)=16),
    number INTEGER NOT NULL CHECK (number BETWEEN -2147483648 AND 2147483647),
    title TEXT NOT NULL CHECK (length(trim(title))>=1 AND length(title)<=500),
    type TEXT NOT NULL DEFAULT 'task' CHECK (type IN ('task','bug','story','epic','subtask')),
    priority TEXT NOT NULL DEFAULT 'none' CHECK (priority IN ('none','low','medium','high','urgent')),
    status_id BLOB NOT NULL CHECK (typeof(status_id)='blob' AND length(status_id)=16),
    start_date TEXT CHECK (start_date IS NULL OR (length(start_date)=10 AND start_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'
        AND start_date BETWEEN '0001-01-01' AND '9999-12-31' AND coalesce(date(start_date,'+0 days')=start_date,0))),
    due_date TEXT CHECK (due_date IS NULL OR (length(due_date)=10 AND due_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'
        AND due_date BETWEEN '0001-01-01' AND '9999-12-31' AND coalesce(date(due_date,'+0 days')=due_date,0))),
    due_at INTEGER,
    estimate TEXT CHECK (estimate IS NULL OR (
        length(estimate)>0 AND estimate NOT GLOB '*[^0-9.+-]*'
        AND (CASE WHEN substr(estimate,1,1) IN ('+','-') THEN substr(estimate,2) ELSE estimate END) GLOB '[0-9]*'
        AND (CASE WHEN substr(estimate,1,1) IN ('+','-') THEN substr(estimate,2) ELSE estimate END) NOT GLOB '*[^0-9.]*'
        AND substr(estimate,-1)<>'.' AND instr(substr(estimate,instr(estimate,'.')+1),'.')=0)),
    parent_id BLOB CHECK (parent_id IS NULL OR (typeof(parent_id)='blob' AND length(parent_id)=16)),
    milestone_id BLOB CHECK (milestone_id IS NULL OR (typeof(milestone_id)='blob' AND length(milestone_id)=16)),
    recurrence TEXT CHECK (recurrence IS NULL OR json_valid(recurrence)),
    sort_key TEXT NOT NULL DEFAULT 'V',
    schema_version INTEGER NOT NULL DEFAULT 2 CHECK (schema_version BETWEEN -2147483648 AND 2147483647),
    content_json TEXT NOT NULL CHECK (json_valid(content_json)),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version BETWEEN -2147483648 AND 2147483647),
    archived_at INTEGER,
    deleted_at INTEGER,
    created_by BLOB NOT NULL REFERENCES users(id) CHECK (typeof(created_by)='blob' AND length(created_by)=16),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    text TEXT NOT NULL DEFAULT '',
    chosung TEXT NOT NULL DEFAULT '',
    estimate_unit TEXT,
    UNIQUE (workspace_id,id), UNIQUE (workspace_id,project_id,number),
    FOREIGN KEY (workspace_id,project_id) REFERENCES projects(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,project_id,status_id) REFERENCES statuses(workspace_id,project_id,id),
    FOREIGN KEY (workspace_id,parent_id) REFERENCES tasks(workspace_id,id),
    FOREIGN KEY (workspace_id,milestone_id) REFERENCES milestones(workspace_id,id),
    -- Integer casts only after decimal shape and exact integral/range checks.
    CHECK (estimate_unit IS NULL OR (estimate_unit='minutes' AND estimate IS NOT NULL
        AND (instr(estimate,'.')=0 OR substr(estimate,instr(estimate,'.')+1) NOT GLOB '*[^0]*')
        AND (substr(estimate,1,1)<>'-' OR replace(replace(estimate,'0',''),'.','')='-')
        AND length(ltrim(ltrim(CASE WHEN instr(estimate,'.')>0 THEN substr(estimate,1,instr(estimate,'.')-1) ELSE estimate END,'+-'),'0'))<=10
        AND CAST(estimate AS INTEGER) BETWEEN 0 AND 2147483647))
) STRICT;
CREATE INDEX tasks_workspace_project_idx ON tasks(workspace_id,project_id) WHERE deleted_at IS NULL;
CREATE INDEX tasks_workspace_id_milestone_id_idx ON tasks(workspace_id,milestone_id);

CREATE TABLE groups (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    name TEXT NOT NULL CHECK (length(trim(name))>=1 AND length(name)<=100),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id)
) STRICT;
CREATE TABLE group_members (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    group_id BLOB NOT NULL CHECK (typeof(group_id)='blob' AND length(group_id)=16),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (workspace_id,group_id,user_id),
    FOREIGN KEY (workspace_id,group_id) REFERENCES groups(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX group_members_workspace_id_user_id_idx ON group_members(workspace_id,user_id);
CREATE TABLE project_members (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    project_id BLOB NOT NULL CHECK (typeof(project_id)='blob' AND length(project_id)=16),
    user_id BLOB CHECK (user_id IS NULL OR (typeof(user_id)='blob' AND length(user_id)=16)),
    group_id BLOB CHECK (group_id IS NULL OR (typeof(group_id)='blob' AND length(group_id)=16)),
    role TEXT NOT NULL CHECK (role IN ('lead','member','viewer')),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id), CHECK ((user_id IS NULL)<>(group_id IS NULL)),
    FOREIGN KEY (workspace_id,project_id) REFERENCES projects(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,group_id) REFERENCES groups(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE UNIQUE INDEX project_members_user_unique ON project_members(workspace_id,project_id,user_id);
CREATE UNIQUE INDEX project_members_group_unique ON project_members(workspace_id,project_id,group_id);
CREATE INDEX project_members_workspace_user_idx ON project_members(workspace_id,user_id);
CREATE INDEX project_members_workspace_id_group_id_idx ON project_members(workspace_id,group_id) WHERE group_id IS NOT NULL;
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
CREATE UNIQUE INDEX document_members_user_unique ON document_members(workspace_id,document_id,user_id);
CREATE UNIQUE INDEX document_members_group_unique ON document_members(workspace_id,document_id,group_id);
CREATE INDEX document_members_workspace_id_user_id_idx ON document_members(workspace_id,user_id);
CREATE INDEX document_members_workspace_id_group_id_idx ON document_members(workspace_id,group_id) WHERE group_id IS NOT NULL;
CREATE TABLE labels (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    project_id BLOB NOT NULL CHECK (typeof(project_id)='blob' AND length(project_id)=16),
    name TEXT NOT NULL,
    color TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id),
    FOREIGN KEY (workspace_id,project_id) REFERENCES projects(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE INDEX labels_workspace_id_project_id_idx ON labels(workspace_id,project_id);
CREATE TABLE task_assignees (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    PRIMARY KEY (task_id,user_id),
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX task_assignees_user_id_idx ON task_assignees(user_id);
CREATE INDEX task_assignees_workspace_id_task_id_idx ON task_assignees(workspace_id,task_id);
CREATE INDEX task_assignees_workspace_id_user_id_idx ON task_assignees(workspace_id,user_id);
CREATE TABLE task_labels (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    label_id BLOB NOT NULL CHECK (typeof(label_id)='blob' AND length(label_id)=16),
    PRIMARY KEY (task_id,label_id),
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,label_id) REFERENCES labels(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE INDEX task_labels_workspace_id_label_id_idx ON task_labels(workspace_id,label_id);
CREATE INDEX task_labels_workspace_id_task_id_idx ON task_labels(workspace_id,task_id);
CREATE TABLE milestones (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    project_id BLOB NOT NULL CHECK (typeof(project_id)='blob' AND length(project_id)=16),
    name TEXT NOT NULL,
    due_date TEXT CHECK (due_date IS NULL OR (length(due_date)=10 AND due_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'
        AND due_date BETWEEN '0001-01-01' AND '9999-12-31' AND coalesce(date(due_date,'+0 days')=due_date,0))),
    sort_key TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id),
    FOREIGN KEY (workspace_id,project_id) REFERENCES projects(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE INDEX milestones_workspace_id_project_id_idx ON milestones(workspace_id,project_id);
CREATE TABLE task_dependencies (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    blocker_id BLOB NOT NULL CHECK (typeof(blocker_id)='blob' AND length(blocker_id)=16),
    blocked_id BLOB NOT NULL CHECK (typeof(blocked_id)='blob' AND length(blocked_id)=16),
    type TEXT NOT NULL DEFAULT 'FS' CHECK (type IN ('FS','SS','FF')),
    lag_days INTEGER NOT NULL DEFAULT 0 CHECK (lag_days BETWEEN 0 AND 2147483647),
    PRIMARY KEY (blocker_id,blocked_id), CHECK (blocker_id<>blocked_id),
    FOREIGN KEY (workspace_id,blocker_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,blocked_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE INDEX task_dependencies_workspace_id_blocked_id_idx ON task_dependencies(workspace_id,blocked_id);
CREATE INDEX task_dependencies_workspace_id_blocker_id_idx ON task_dependencies(workspace_id,blocker_id);

-- PG006,007,014,030,031: attachment parent, native bytes, worker leases/files.
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
CREATE INDEX attachments_workspace_id_document_id_idx ON attachments(workspace_id,document_id);
CREATE INDEX attachments_workspace_id_task_id_idx ON attachments(workspace_id,task_id);
CREATE INDEX attachments_uploader_id_idx ON attachments(uploader_id);
CREATE INDEX attachments_uploading_created_at_idx ON attachments(status,created_at) WHERE status IN ('uploading','assembling');
CREATE INDEX attachments_uploader_id_stored_idx ON attachments(uploader_id,created_at,id) WHERE status='stored';
CREATE INDEX attachments_extract_pending_idx ON attachments(completed_at,id) WHERE status='stored' AND extract_status='pending';
CREATE INDEX attachments_preview_pending_idx ON attachments(completed_at,id) WHERE status='stored' AND preview_status='pending';
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
CREATE INDEX attachment_text_workspace_attachment_idx ON attachment_text(workspace_id,attachment_id);
CREATE INDEX attachment_text_pending_embedding_idx ON attachment_text(workspace_id,attachment_id) WHERE embedding IS NULL AND text<>'';
-- No FK: deleted attachment/workspace cleanup identities must survive.
CREATE TABLE attachment_object_cleanups (
    id BLOB PRIMARY KEY NOT NULL DEFAULT (randomblob(16)) CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    attachment_id BLOB NOT NULL CHECK (typeof(attachment_id)='blob' AND length(attachment_id)=16),
    storage_key TEXT NOT NULL,
    due_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 2147483647)
) STRICT;
CREATE INDEX attachment_object_cleanups_due_idx ON attachment_object_cleanups(due_at,id);

-- PG009,011,018..020,022,024,025.
CREATE TABLE invitations (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    email TEXT NOT NULL CHECK (length(email)>0 AND email NOT GLOB '*[^!-~]*' AND email=lower(email)),
    role TEXT NOT NULL CHECK (role IN ('owner','admin','member','guest')),
    token_hash TEXT NOT NULL UNIQUE,
    invited_by BLOB NOT NULL REFERENCES users(id) CHECK (typeof(invited_by)='blob' AND length(invited_by)=16),
    expires_at INTEGER NOT NULL,
    accepted_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE INDEX invitations_workspace_id_invited_by_idx ON invitations(workspace_id,invited_by);
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
CREATE INDEX comments_workspace_id_parent_id_idx ON comments(workspace_id,parent_id);
CREATE INDEX comments_workspace_id_created_by_idx ON comments(workspace_id,created_by,created_at,id);
CREATE INDEX comments_workspace_id_document_id_created_at_idx ON comments(workspace_id,document_id,created_at,id);
CREATE INDEX comments_workspace_id_task_id_created_at_idx ON comments(workspace_id,task_id,created_at,id);
CREATE TABLE notifications (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    event_id BLOB NOT NULL CHECK (typeof(event_id)='blob' AND length(event_id)=16),
    verb TEXT NOT NULL,
    actor_user_id BLOB CHECK (actor_user_id IS NULL OR (typeof(actor_user_id)='blob' AND length(actor_user_id)=16)),
    target_type TEXT,
    target_id BLOB CHECK (target_id IS NULL OR (typeof(target_id)='blob' AND length(target_id)=16)),
    payload TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(payload)),
    read_at INTEGER,
    archived_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id), UNIQUE (workspace_id,user_id,event_id)
) STRICT;
CREATE INDEX notifications_inbox_idx ON notifications(workspace_id,user_id,created_at,id);
CREATE INDEX notifications_unread_idx ON notifications(workspace_id,user_id) WHERE read_at IS NULL AND archived_at IS NULL;
CREATE TABLE notification_prefs (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    in_app INTEGER NOT NULL DEFAULT 1 CHECK (in_app IN (0,1)),
    mail_immediate INTEGER NOT NULL DEFAULT 1 CHECK (mail_immediate IN (0,1)),
    mail_digest INTEGER NOT NULL DEFAULT 0 CHECK (mail_digest IN (0,1)),
    last_digest_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (workspace_id,user_id),
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX notification_prefs_digest_due_idx ON notification_prefs(last_digest_at) WHERE mail_digest=1;
CREATE TABLE workspace_holidays (
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    date TEXT NOT NULL CHECK (length(date)=10 AND date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'
        AND date BETWEEN '0001-01-01' AND '9999-12-31' AND coalesce(date(date,'+0 days')=date,0)),
    PRIMARY KEY (workspace_id,date)
) STRICT;
CREATE TABLE ics_tokens (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    token_hash TEXT NOT NULL UNIQUE,
    expires_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,user_id),
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX ics_tokens_expires_at_idx ON ics_tokens(expires_at);
CREATE TABLE magic_tokens (
    token_hash TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('password_reset','login','email_change')),
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    generation INTEGER NOT NULL CHECK (generation BETWEEN -2147483648 AND 2147483647),
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    new_email TEXT,
    CHECK ((kind='email_change')=(new_email IS NOT NULL)
        AND (new_email IS NULL OR (length(new_email)>0 AND new_email NOT GLOB '*[^!-~]*' AND new_email=lower(new_email))))
) STRICT;
CREATE INDEX magic_tokens_expires_at_idx ON magic_tokens(expires_at);
CREATE INDEX magic_tokens_user_id_idx ON magic_tokens(user_id);
CREATE TABLE task_activity (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    actor_user_id BLOB REFERENCES users(id) ON DELETE SET NULL CHECK (actor_user_id IS NULL OR (typeof(actor_user_id)='blob' AND length(actor_user_id)=16)),
    channel TEXT NOT NULL CHECK (channel IN ('web','api','mcp','webhook','system')),
    kind TEXT NOT NULL CHECK (kind IN ('created','changed')),
    changes TEXT NOT NULL CHECK (json_valid(changes) AND json_type(changes)='array' AND json_array_length(changes)<=14 AND (kind='created' OR json_array_length(changes)>0)),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE INDEX task_activity_workspace_task_created_idx ON task_activity(workspace_id,task_id,created_at DESC,id DESC);
CREATE INDEX task_activity_actor_idx ON task_activity(actor_user_id);
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
CREATE UNIQUE INDEX stars_user_document_unique ON stars(workspace_id,user_id,document_id);
CREATE UNIQUE INDEX stars_user_task_unique ON stars(workspace_id,user_id,task_id);
CREATE INDEX stars_workspace_id_document_id_idx ON stars(workspace_id,document_id);
CREATE INDEX stars_workspace_id_task_id_idx ON stars(workspace_id,task_id);
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
CREATE INDEX share_links_workspace_id_document_id_idx ON share_links(workspace_id,document_id);
CREATE INDEX share_links_workspace_id_project_id_idx ON share_links(workspace_id,project_id);
CREATE INDEX share_links_workspace_id_user_id_idx ON share_links(workspace_id,user_id);

-- PG013,041: fresh installs remain unseeded; named ensure_consumer creates rows.
-- PostgreSQL-only last_xact is omitted from the family sequence cursor.
CREATE TABLE outbox_consumers (
    consumer TEXT PRIMARY KEY NOT NULL CHECK (length(consumer) BETWEEN 1 AND 63 AND consumer GLOB '[a-z]*' AND consumer NOT GLOB '*[^a-z0-9_-]*'),
    last_seq INTEGER NOT NULL DEFAULT 0 CHECK (typeof(last_seq)='integer' AND last_seq>=0),
    lease_owner BLOB CHECK (lease_owner IS NULL OR (typeof(lease_owner)='blob' AND length(lease_owner)=16)),
    lease_until INTEGER,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    CHECK ((lease_owner IS NULL)=(lease_until IS NULL))
) STRICT;
CREATE TABLE outbox_failures (
    consumer TEXT NOT NULL,
    event_id BLOB NOT NULL CHECK (typeof(event_id)='blob' AND length(event_id)=16),
    attempts INTEGER NOT NULL CHECK (attempts BETWEEN 1 AND 2147483647),
    last_error TEXT NOT NULL,
    next_attempt_at INTEGER NOT NULL,
    dead_at INTEGER,
    skipped_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (consumer,event_id)
) STRICT;
CREATE INDEX outbox_failures_retry_idx ON outbox_failures(consumer,next_attempt_at) WHERE dead_at IS NULL;
CREATE TABLE processed_events (
    consumer TEXT NOT NULL,
    event_id BLOB NOT NULL CHECK (typeof(event_id)='blob' AND length(event_id)=16),
    processed_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (consumer,event_id)
) STRICT;

-- PG026: legal evidence and sealed settings; immutable-publish policy in Rust.
CREATE TABLE legal_documents (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    kind TEXT NOT NULL CHECK (length(kind) BETWEEN 1 AND 50 AND kind NOT GLOB '*[^a-z0-9-]*'),
    version INTEGER NOT NULL CHECK (version BETWEEN 1 AND 2147483647),
    title TEXT NOT NULL,
    body_markdown TEXT NOT NULL,
    body_html TEXT NOT NULL,
    required INTEGER NOT NULL CHECK (required IN (0,1)),
    effective_at INTEGER NOT NULL,
    published_at INTEGER NOT NULL,
    created_by BLOB NOT NULL REFERENCES users(id) CHECK (typeof(created_by)='blob' AND length(created_by)=16),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (kind,version)
) STRICT;
CREATE TABLE user_consents (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    user_id BLOB NOT NULL REFERENCES users(id) CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    kind TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version BETWEEN -2147483648 AND 2147483647),
    consented_at INTEGER NOT NULL,
    ip TEXT,
    channel TEXT NOT NULL CHECK (channel IN ('signup','gate')),
    UNIQUE (user_id,kind,version)
) STRICT;
CREATE TABLE instance_settings (
    key TEXT PRIMARY KEY NOT NULL CHECK (length(key) BETWEEN 1 AND 64 AND key GLOB '[A-Za-z]*' AND key NOT GLOB '*[^A-Za-z0-9.]*'),
    value TEXT NOT NULL CHECK (json_valid(value)),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE TABLE instance_settings_meta (
    id INT PRIMARY KEY NOT NULL DEFAULT 1 CHECK (id=1),
    revision INTEGER NOT NULL DEFAULT 0
) STRICT;
INSERT INTO instance_settings_meta(id,revision) VALUES (1,0);

-- PG029,035..038,044: encrypted MFA/OIDC and pinned issuer identity.
CREATE TABLE user_mfa (
    user_id BLOB PRIMARY KEY NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    totp_secret TEXT NOT NULL CHECK (substr(totp_secret,1,7)='enc:v2:'),
    enabled_at INTEGER,
    recovery_hashes TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(recovery_hashes) AND json_type(recovery_hashes)='array'),
    last_used_step INTEGER CHECK (last_used_step BETWEEN -2147483648 AND 2147483647),
    verify_window_start INTEGER,
    verify_count INTEGER NOT NULL DEFAULT 0 CHECK (verify_count BETWEEN -2147483648 AND 2147483647),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE TABLE identity_links (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    user_id BLOB NOT NULL REFERENCES users(id) CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    provider TEXT NOT NULL CHECK (provider IN ('google','microsoft','kakao','naver','generic')),
    provider_user_id TEXT NOT NULL,
    email TEXT CHECK (email IS NULL OR (length(email)>0 AND email NOT GLOB '*[^!-~]*' AND email=lower(email))),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    issuer TEXT CHECK (issuer IS NULL OR issuer<>''),
    UNIQUE (provider,provider_user_id), UNIQUE (user_id,provider)
) STRICT;
CREATE TABLE workspace_oidc (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL UNIQUE REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    issuer TEXT NOT NULL,
    client_id TEXT NOT NULL,
    client_secret TEXT NOT NULL CHECK (substr(client_secret,1,7)='enc:v2:'),
    label TEXT NOT NULL DEFAULT 'SSO',
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE TABLE mfa_challenges (
    token_hash TEXT PRIMARY KEY NOT NULL,
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    generation INTEGER NOT NULL CHECK (generation BETWEEN -2147483648 AND 2147483647),
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE INDEX mfa_challenges_expires_at_idx ON mfa_challenges(expires_at);
CREATE INDEX mfa_challenges_user_id_idx ON mfa_challenges(user_id);
CREATE TABLE oidc_states (
    state_hash TEXT PRIMARY KEY NOT NULL,
    payload TEXT NOT NULL CHECK (substr(payload,1,7)='enc:v2:'),
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE INDEX oidc_states_expires_at_idx ON oidc_states(expires_at);

-- PG027,040: integration/push delivery locators do not gain event FKs.
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
CREATE INDEX webhooks_created_by_idx ON webhooks(created_by);
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
CREATE INDEX webhook_deliveries_pending_next_attempt_at_idx ON webhook_deliveries(next_attempt_at) WHERE status='pending';
CREATE INDEX webhook_deliveries_settled_created_at_idx ON webhook_deliveries(created_at) WHERE status IN ('delivered','failed');
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
CREATE INDEX github_install_states_workspace_id_idx ON github_install_states(workspace_id);
CREATE INDEX github_install_states_expires_at_idx ON github_install_states(expires_at);
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
CREATE INDEX github_deliveries_processed_at_idx ON github_deliveries(processed_at);
CREATE TABLE instance_config (
    id INT PRIMARY KEY NOT NULL CHECK (id=1),
    vapid_public_key TEXT,
    vapid_private_key TEXT CHECK (vapid_private_key IS NULL OR substr(vapid_private_key,1,7)='enc:v2:')
) STRICT;
INSERT INTO instance_config(id) VALUES (1);
CREATE TABLE push_subscriptions (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    endpoint TEXT NOT NULL CHECK (length(CAST(endpoint AS BLOB))<=2048),
    p256dh TEXT NOT NULL CHECK (length(p256dh)=87),
    auth TEXT NOT NULL CHECK (length(auth)=22),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    session_id BLOB NOT NULL REFERENCES sessions(id) ON DELETE CASCADE CHECK (typeof(session_id)='blob' AND length(session_id)=16),
    UNIQUE (user_id,endpoint)
) STRICT;
CREATE INDEX push_subscriptions_user_updated_idx ON push_subscriptions(user_id,updated_at DESC);
CREATE INDEX push_subscriptions_endpoint_idx ON push_subscriptions(endpoint);
CREATE INDEX push_subscriptions_session_idx ON push_subscriptions(session_id);
CREATE TABLE push_deliveries (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    event_id BLOB NOT NULL CHECK (typeof(event_id)='blob' AND length(event_id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    subscription_id BLOB NOT NULL REFERENCES push_subscriptions(id) ON DELETE CASCADE CHECK (typeof(subscription_id)='blob' AND length(subscription_id)=16),
    attempt INTEGER NOT NULL DEFAULT 0 CHECK (attempt BETWEEN -2147483648 AND 2147483647),
    claimed_until INTEGER,
    handed_off_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (event_id,subscription_id)
) STRICT;
CREATE INDEX push_deliveries_subscription_idx ON push_deliveries(subscription_id);

-- Engine-bound step receipts: populated only by the actual migration runner.
CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY NOT NULL CHECK (version>0),
    lineage TEXT NOT NULL CHECK (lineage='fvoci-sqlite-current-v1'),
    sql_sha256 TEXT NOT NULL CHECK (length(sql_sha256)=64 AND sql_sha256 NOT GLOB '*[^0-9a-f]*'),
    applied_at INTEGER NOT NULL
) STRICT;

-- PG028: typed collections; SQL decimal sort/filter requires named exact math.
CREATE TABLE document_tags (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    name TEXT NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 100),
    color TEXT NOT NULL CHECK (color IN ('gray','red','orange','amber','green','teal','blue','violet','pink')),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id)
) STRICT;
-- SQLite lower enforces the existing ASCII invariant. Full Unicode lower(name)
-- equivalence remains a required consumer/normalized-key decision (W2), not a
-- claimed SQLite BINARY substitute for PostgreSQL locale-aware lower().
CREATE UNIQUE INDEX document_tags_workspace_id_lower_name_idx ON document_tags(workspace_id,lower(name));
CREATE TABLE document_tag_assignments (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    document_id BLOB NOT NULL CHECK (typeof(document_id)='blob' AND length(document_id)=16),
    tag_id BLOB NOT NULL CHECK (typeof(tag_id)='blob' AND length(tag_id)=16),
    PRIMARY KEY (workspace_id,document_id,tag_id),
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,tag_id) REFERENCES document_tags(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE INDEX document_tag_assignments_workspace_id_tag_id_idx ON document_tag_assignments(workspace_id,tag_id);
CREATE TABLE collections (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    project_id BLOB CHECK (project_id IS NULL OR (typeof(project_id)='blob' AND length(project_id)=16)),
    kind TEXT NOT NULL CHECK (kind IN ('document','task') AND (kind<>'task' OR project_id IS NOT NULL)),
    name TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1 CHECK (version BETWEEN 1 AND 2147483647),
    deleted_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id),
    FOREIGN KEY (workspace_id,project_id) REFERENCES projects(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE UNIQUE INDEX collections_task_project_unique ON collections(workspace_id,project_id) WHERE kind='task';
CREATE INDEX collections_scope_idx ON collections(workspace_id,project_id,kind);
CREATE TABLE collection_items (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    collection_id BLOB NOT NULL CHECK (typeof(collection_id)='blob' AND length(collection_id)=16),
    document_id BLOB CHECK (document_id IS NULL OR (typeof(document_id)='blob' AND length(document_id)=16)),
    task_id BLOB CHECK (task_id IS NULL OR (typeof(task_id)='blob' AND length(task_id)=16)),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version BETWEEN 1 AND 2147483647),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,collection_id,id), CHECK ((document_id IS NULL)<>(task_id IS NULL)),
    FOREIGN KEY (workspace_id,collection_id) REFERENCES collections(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE UNIQUE INDEX collection_items_document_unique ON collection_items(workspace_id,document_id) WHERE document_id IS NOT NULL;
CREATE UNIQUE INDEX collection_items_task_unique ON collection_items(workspace_id,task_id) WHERE task_id IS NOT NULL;
CREATE INDEX collection_items_collection_idx ON collection_items(workspace_id,collection_id,id);
CREATE TABLE collection_fields (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    collection_id BLOB NOT NULL CHECK (typeof(collection_id)='blob' AND length(collection_id)=16),
    key TEXT NOT NULL CHECK (length(key) BETWEEN 1 AND 50 AND key GLOB '[a-z]*' AND key NOT GLOB '*[^a-z0-9_]*'),
    name TEXT NOT NULL,
    description TEXT,
    type TEXT NOT NULL CHECK (type IN ('text','paragraph','number','date','datetime','checkbox','select','multi_select','checkboxes','user','user_multi','labels')),
    sort_key TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1 CHECK (version BETWEEN 1 AND 2147483647),
    deleted_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,collection_id,id), UNIQUE (collection_id,key), UNIQUE (workspace_id,collection_id,id,type),
    FOREIGN KEY (workspace_id,collection_id) REFERENCES collections(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE collection_options (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    collection_id BLOB NOT NULL CHECK (typeof(collection_id)='blob' AND length(collection_id)=16),
    field_id BLOB NOT NULL CHECK (typeof(field_id)='blob' AND length(field_id)=16),
    key TEXT NOT NULL,
    label TEXT NOT NULL,
    sort_key TEXT NOT NULL,
    deleted_at INTEGER,
    UNIQUE (workspace_id,collection_id,field_id,id), UNIQUE (field_id,key),
    FOREIGN KEY (workspace_id,collection_id,field_id) REFERENCES collection_fields(workspace_id,collection_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE collection_values (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    collection_id BLOB NOT NULL CHECK (typeof(collection_id)='blob' AND length(collection_id)=16),
    item_id BLOB NOT NULL CHECK (typeof(item_id)='blob' AND length(item_id)=16),
    field_id BLOB NOT NULL CHECK (typeof(field_id)='blob' AND length(field_id)=16),
    field_type TEXT NOT NULL,
    value_text TEXT,
    value_number TEXT CHECK (value_number IS NULL OR (
        length(value_number)>0 AND value_number NOT GLOB '*[^0-9.+-]*'
        AND (CASE WHEN substr(value_number,1,1) IN ('+','-') THEN substr(value_number,2) ELSE value_number END) GLOB '[0-9]*'
        AND (CASE WHEN substr(value_number,1,1) IN ('+','-') THEN substr(value_number,2) ELSE value_number END) NOT GLOB '*[^0-9.]*'
        AND substr(value_number,-1)<>'.' AND instr(substr(value_number,instr(value_number,'.')+1),'.')=0)),
    value_date TEXT CHECK (value_date IS NULL OR (length(value_date)=10 AND value_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'
        AND value_date BETWEEN '0001-01-01' AND '9999-12-31' AND coalesce(date(value_date,'+0 days')=value_date,0))),
    value_ts INTEGER,
    value_bool INTEGER CHECK (value_bool IN (0,1)),
    PRIMARY KEY (workspace_id,collection_id,item_id,field_id),
    FOREIGN KEY (workspace_id,collection_id,item_id) REFERENCES collection_items(workspace_id,collection_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,collection_id,field_id,field_type) REFERENCES collection_fields(workspace_id,collection_id,id,type) ON DELETE CASCADE,
    CHECK ((value_text IS NOT NULL)+(value_number IS NOT NULL)+(value_date IS NOT NULL)+(value_ts IS NOT NULL)+(value_bool IS NOT NULL)=1
        AND ((field_type IN ('text','paragraph') AND value_text IS NOT NULL)
            OR (field_type='number' AND value_number IS NOT NULL)
            OR (field_type='date' AND value_date IS NOT NULL)
            OR (field_type='datetime' AND value_ts IS NOT NULL)
            OR (field_type='checkbox' AND value_bool IS NOT NULL)))
) STRICT;
CREATE INDEX collection_values_date_idx ON collection_values(workspace_id,collection_id,field_id,value_date,item_id);
CREATE INDEX collection_values_ts_idx ON collection_values(workspace_id,collection_id,field_id,value_ts,item_id);
-- A scope lookup index ONLY. Its TEXT order must never implement numeric sort.
CREATE INDEX collection_values_number_idx ON collection_values(workspace_id,collection_id,field_id,value_number,item_id);
CREATE INDEX collection_values_item_idx ON collection_values(workspace_id,item_id);
CREATE TABLE collection_choices (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    collection_id BLOB NOT NULL CHECK (typeof(collection_id)='blob' AND length(collection_id)=16),
    item_id BLOB NOT NULL CHECK (typeof(item_id)='blob' AND length(item_id)=16),
    field_id BLOB NOT NULL CHECK (typeof(field_id)='blob' AND length(field_id)=16),
    field_type TEXT NOT NULL CHECK (field_type IN ('select','multi_select','checkboxes','labels')),
    option_id BLOB NOT NULL CHECK (typeof(option_id)='blob' AND length(option_id)=16),
    PRIMARY KEY (workspace_id,collection_id,item_id,field_id,option_id),
    FOREIGN KEY (workspace_id,collection_id,item_id) REFERENCES collection_items(workspace_id,collection_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,collection_id,field_id,field_type) REFERENCES collection_fields(workspace_id,collection_id,id,type) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,collection_id,field_id,option_id) REFERENCES collection_options(workspace_id,collection_id,field_id,id) ON DELETE CASCADE
) STRICT;
CREATE UNIQUE INDEX collection_choices_single_unique ON collection_choices(workspace_id,item_id,field_id) WHERE field_type='select';
CREATE INDEX collection_choices_option_idx ON collection_choices(workspace_id,collection_id,field_id,option_id,item_id);
CREATE INDEX collection_choices_item_idx ON collection_choices(workspace_id,item_id);
CREATE TABLE collection_people (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    collection_id BLOB NOT NULL CHECK (typeof(collection_id)='blob' AND length(collection_id)=16),
    item_id BLOB NOT NULL CHECK (typeof(item_id)='blob' AND length(item_id)=16),
    field_id BLOB NOT NULL CHECK (typeof(field_id)='blob' AND length(field_id)=16),
    field_type TEXT NOT NULL CHECK (field_type IN ('user','user_multi')),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    PRIMARY KEY (workspace_id,collection_id,item_id,field_id,user_id),
    FOREIGN KEY (workspace_id,collection_id,item_id) REFERENCES collection_items(workspace_id,collection_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,collection_id,field_id,field_type) REFERENCES collection_fields(workspace_id,collection_id,id,type) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE
) STRICT;
CREATE UNIQUE INDEX collection_people_single_unique ON collection_people(workspace_id,item_id,field_id) WHERE field_type='user';
CREATE INDEX collection_people_user_idx ON collection_people(workspace_id,user_id,collection_id,field_id,item_id);
CREATE INDEX collection_people_item_idx ON collection_people(workspace_id,item_id);
CREATE TABLE collection_views (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    collection_id BLOB NOT NULL CHECK (typeof(collection_id)='blob' AND length(collection_id)=16),
    owner_id BLOB NOT NULL CHECK (typeof(owner_id)='blob' AND length(owner_id)=16),
    visibility TEXT NOT NULL CHECK (visibility IN ('private','shared')),
    name TEXT NOT NULL,
    type TEXT NOT NULL CHECK (type IN ('table','board','calendar')),
    config TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(config) AND length(CAST(config AS BLOB))<=262144),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version BETWEEN 1 AND 2147483647),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    FOREIGN KEY (workspace_id,collection_id) REFERENCES collections(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,owner_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX collection_views_collection_idx ON collection_views(workspace_id,collection_id,owner_id);
CREATE TABLE views (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    project_id BLOB NOT NULL CHECK (typeof(project_id)='blob' AND length(project_id)=16),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    name TEXT NOT NULL,
    type TEXT NOT NULL CHECK (type IN ('list','board','calendar','gantt','table')),
    config TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(config) AND length(CAST(config AS BLOB))<=262144),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    FOREIGN KEY (workspace_id,project_id) REFERENCES projects(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX views_workspace_id_project_id_user_id_idx ON views(workspace_id,project_id,user_id);
CREATE INDEX views_workspace_id_user_id_project_id_calendar_idx ON views(workspace_id,user_id,project_id) WHERE type='calendar';

-- PG028 scope and field-identity triggers, each INSERT/UPDATE variant explicit.
CREATE TRIGGER collection_items_scope_insert BEFORE INSERT ON collection_items BEGIN
    SELECT RAISE(ABORT,'collection scope mismatch') WHERE NOT EXISTS (
        SELECT 1 FROM collections c
        LEFT JOIN documents d ON d.workspace_id=NEW.workspace_id AND d.id=NEW.document_id
        LEFT JOIN tasks t ON t.workspace_id=NEW.workspace_id AND t.id=NEW.task_id
        WHERE c.workspace_id=NEW.workspace_id AND c.id=NEW.collection_id
        AND ((NEW.document_id IS NOT NULL AND c.kind='document' AND d.id IS NOT NULL AND c.project_id IS d.project_id)
            OR (NEW.task_id IS NOT NULL AND c.kind='task' AND t.id IS NOT NULL AND c.project_id IS t.project_id)));
END;
CREATE TRIGGER collection_items_scope_update BEFORE UPDATE OF collection_id,document_id,task_id,workspace_id ON collection_items BEGIN
    SELECT RAISE(ABORT,'collection scope mismatch') WHERE NOT EXISTS (
        SELECT 1 FROM collections c
        LEFT JOIN documents d ON d.workspace_id=NEW.workspace_id AND d.id=NEW.document_id
        LEFT JOIN tasks t ON t.workspace_id=NEW.workspace_id AND t.id=NEW.task_id
        WHERE c.workspace_id=NEW.workspace_id AND c.id=NEW.collection_id
        AND ((NEW.document_id IS NOT NULL AND c.kind='document' AND d.id IS NOT NULL AND c.project_id IS d.project_id)
            OR (NEW.task_id IS NOT NULL AND c.kind='task' AND t.id IS NOT NULL AND c.project_id IS t.project_id)));
END;
CREATE TRIGGER collections_keep_scope BEFORE UPDATE OF workspace_id,project_id,kind ON collections
WHEN (NEW.workspace_id IS NOT OLD.workspace_id OR NEW.project_id IS NOT OLD.project_id OR NEW.kind IS NOT OLD.kind)
    AND EXISTS (SELECT 1 FROM collection_items WHERE collection_id=OLD.id) BEGIN
    SELECT RAISE(ABORT,'populated collection scope is immutable');
END;
CREATE TRIGGER documents_keep_collection_scope BEFORE UPDATE OF workspace_id,project_id ON documents
WHEN (NEW.workspace_id IS NOT OLD.workspace_id OR NEW.project_id IS NOT OLD.project_id)
    AND EXISTS (SELECT 1 FROM collection_items WHERE document_id=OLD.id) BEGIN
    SELECT RAISE(ABORT,'detach document before changing scope');
END;
CREATE TRIGGER tasks_keep_collection_scope BEFORE UPDATE OF workspace_id,project_id ON tasks
WHEN (NEW.workspace_id IS NOT OLD.workspace_id OR NEW.project_id IS NOT OLD.project_id)
    AND EXISTS (SELECT 1 FROM collection_items WHERE task_id=OLD.id) BEGIN
    SELECT RAISE(ABORT,'detach task before changing scope');
END;
CREATE TRIGGER collection_fields_identity BEFORE UPDATE ON collection_fields
WHEN NEW.workspace_id IS NOT OLD.workspace_id OR NEW.collection_id IS NOT OLD.collection_id
    OR NEW.key IS NOT OLD.key OR NEW.type IS NOT OLD.type BEGIN
    SELECT RAISE(ABORT,'field identity is immutable');
END;

-- PG023,033,046: import journal including current native archive bindings.
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
CREATE INDEX import_jobs_lease_idx ON import_jobs(status,lease_until);
CREATE INDEX import_jobs_workspace_id_idx ON import_jobs(workspace_id);
CREATE UNIQUE INDEX import_jobs_native_request_unique ON import_jobs(workspace_id,created_by,native_request_id) WHERE source='native-archive';
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
CREATE INDEX import_deferred_events_job_seq_idx ON import_deferred_events(workspace_id,import_job_id,seq);
-- PG033 events_defer_import needs the caller's current import context: the
-- named emitter routes to events/deferred_events and checks running job in tx.

-- PG037,039: task canonical native history and template/origin metadata.
CREATE TABLE task_states (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    state BLOB NOT NULL,
    encoding INTEGER NOT NULL DEFAULT 1 CHECK (encoding=1),
    compacted_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    writer_generation INTEGER NOT NULL DEFAULT 0,
    snapshot_cutoff_seq INTEGER NOT NULL DEFAULT 0,
    tail_seq INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (workspace_id,task_id),
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE task_collab_updates (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    seq INTEGER NOT NULL,
    op_id BLOB NOT NULL CHECK (typeof(op_id)='blob' AND length(op_id)=16),
    payload BLOB NOT NULL CHECK (length(payload) BETWEEN 1 AND 8388608),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (workspace_id,task_id,seq), UNIQUE (workspace_id,task_id,op_id),
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE TABLE task_collab_op_receipts (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    op_id BLOB NOT NULL CHECK (typeof(op_id)='blob' AND length(op_id)=16),
    seq INTEGER NOT NULL,
    payload_len INTEGER NOT NULL CHECK (payload_len BETWEEN 1 AND 8388608),
    payload_sha256 BLOB NOT NULL CHECK (length(payload_sha256)=32),
    actor_user_id BLOB NOT NULL REFERENCES users(id) CHECK (typeof(actor_user_id)='blob' AND length(actor_user_id)=16),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (workspace_id,task_id,op_id),
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE INDEX task_collab_op_receipts_lookup_idx ON task_collab_op_receipts(workspace_id,task_id,seq);
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
CREATE INDEX task_origins_document_task_idx ON task_origins(workspace_id,document_id,task_id);
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
CREATE INDEX templates_workspace_id_idx ON templates(workspace_id);
CREATE INDEX templates_created_by_idx ON templates(created_by);

-- PG045/047: purge clears live personal-input locators; transfer locators are
-- durable historical IDs (deliberately no destination/document/session FK).
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
CREATE TRIGGER personal_input_document_purge BEFORE DELETE ON documents BEGIN
    UPDATE personal_input_commands SET document_id=NULL WHERE workspace_id=OLD.workspace_id AND document_id=OLD.id;
END;
CREATE TRIGGER personal_input_task_purge BEFORE DELETE ON tasks BEGIN
    UPDATE personal_input_commands SET task_id=NULL WHERE workspace_id=OLD.workspace_id AND task_id=OLD.id;
END;
CREATE TRIGGER personal_input_project_purge BEFORE DELETE ON projects BEGIN
    UPDATE personal_input_commands SET project_id=NULL WHERE workspace_id=OLD.workspace_id AND project_id=OLD.id;
END;
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

-- PG034,048,052..054: immutable historical run/receipt/audit locators.
CREATE TABLE time_entries (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    user_id BLOB NOT NULL REFERENCES users(id) CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    duration_seconds INTEGER CHECK (duration_seconds BETWEEN -2147483648 AND 2147483647),
    note TEXT CHECK (note IS NULL OR length(note)<=2000),
    UNIQUE (workspace_id,id),
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE,
    CHECK (ended_at IS NULL OR ended_at>started_at),
    CHECK ((ended_at IS NULL AND duration_seconds IS NULL)
        OR (ended_at IS NOT NULL AND duration_seconds IS NOT NULL AND duration_seconds>0
            AND ended_at/1000000-started_at/1000000
                + ((ended_at%1000000)-(started_at%1000000))/1000000
                - CASE WHEN (ended_at%1000000)<(started_at%1000000) THEN 1 ELSE 0 END = duration_seconds))
) STRICT;
CREATE UNIQUE INDEX time_entries_one_open_per_actor ON time_entries(workspace_id,user_id) WHERE ended_at IS NULL;
CREATE INDEX time_entries_workspace_id_task_id_started_at_idx ON time_entries(workspace_id,task_id,started_at);
CREATE TABLE task_timer_runs (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    status TEXT NOT NULL CHECK (status IN ('running','paused','stopped')),
    version INTEGER NOT NULL CHECK (version BETWEEN 1 AND 2147483647),
    started_at INTEGER NOT NULL,
    stopped_at INTEGER,
    note TEXT CHECK (note IS NULL OR length(note)<=2000),
    UNIQUE (id,user_id,workspace_id,task_id),
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE,
    CHECK ((status='stopped')=(stopped_at IS NOT NULL)), CHECK (stopped_at IS NULL OR stopped_at>=started_at)
) STRICT;
CREATE UNIQUE INDEX task_timer_one_unfinished_per_person ON task_timer_runs(user_id) WHERE status<>'stopped';
CREATE INDEX task_timer_runs_task ON task_timer_runs(workspace_id,task_id,user_id);
CREATE TABLE task_timer_segments (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    run_id BLOB NOT NULL CHECK (typeof(run_id)='blob' AND length(run_id)=16),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    time_entry_id BLOB UNIQUE CHECK (time_entry_id IS NULL OR (typeof(time_entry_id)='blob' AND length(time_entry_id)=16)),
    FOREIGN KEY (run_id,user_id,workspace_id,task_id) REFERENCES task_timer_runs(id,user_id,workspace_id,task_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,time_entry_id) REFERENCES time_entries(workspace_id,id),
    CHECK (ended_at IS NULL OR ended_at>=started_at), CHECK (time_entry_id IS NULL OR ended_at IS NOT NULL)
) STRICT;
CREATE UNIQUE INDEX task_timer_one_open_segment ON task_timer_segments(run_id) WHERE ended_at IS NULL;
CREATE TABLE task_timer_legacy_open (
    time_entry_id BLOB PRIMARY KEY NOT NULL CHECK (typeof(time_entry_id)='blob' AND length(time_entry_id)=16),
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    FOREIGN KEY (workspace_id,time_entry_id) REFERENCES time_entries(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE
) STRICT;
CREATE INDEX task_timer_legacy_person ON task_timer_legacy_open(user_id);
CREATE TABLE task_timer_commands (
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    request_id BLOB NOT NULL CHECK (typeof(request_id)='blob' AND length(request_id)=16),
    request_hash TEXT NOT NULL CHECK (length(request_hash)=64),
    run_id BLOB CHECK (run_id IS NULL OR (typeof(run_id)='blob' AND length(run_id)=16)),
    result TEXT NOT NULL CHECK (json_valid(result)),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    restored_from_archive BLOB CHECK (restored_from_archive IS NULL OR (typeof(restored_from_archive)='blob' AND length(restored_from_archive)=16)),
    PRIMARY KEY (user_id,request_id)
    -- PG053 deliberately removed run FK; purge must never null this locator.
) STRICT;
CREATE TABLE task_timer_audit (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    request_id BLOB NOT NULL CHECK (typeof(request_id)='blob' AND length(request_id)=16),
    workspace_id BLOB CHECK (workspace_id IS NULL OR (typeof(workspace_id)='blob' AND length(workspace_id)=16)),
    task_id BLOB CHECK (task_id IS NULL OR (typeof(task_id)='blob' AND length(task_id)=16)),
    time_entry_id BLOB CHECK (time_entry_id IS NULL OR (typeof(time_entry_id)='blob' AND length(time_entry_id)=16)),
    verb TEXT NOT NULL,
    before_value TEXT NOT NULL CHECK (json_valid(before_value)),
    after_value TEXT NOT NULL CHECK (json_valid(after_value)),
    reason TEXT NOT NULL CHECK (length(reason) BETWEEN 1 AND 2000),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE INDEX task_timer_audit_user_id_idx ON task_timer_audit(user_id);
CREATE TRIGGER timer_segment_time_entry_purge BEFORE DELETE ON time_entries BEGIN
    UPDATE task_timer_segments SET time_entry_id=NULL WHERE workspace_id=OLD.workspace_id AND time_entry_id=OLD.id;
END;
-- PG048 structural tracking survives. Current self actor/task ACL must still be
-- checked by the named legacy-write operation under the serialized writer tx.
CREATE TRIGGER task_timer_legacy_tracking_insert AFTER INSERT ON time_entries WHEN NEW.ended_at IS NULL BEGIN
    SELECT RAISE(ABORT,'open time entry exists') WHERE
        EXISTS (SELECT 1 FROM task_timer_runs WHERE user_id=NEW.user_id AND status<>'stopped')
        OR EXISTS (SELECT 1 FROM task_timer_legacy_open WHERE user_id=NEW.user_id);
    INSERT INTO task_timer_legacy_open(time_entry_id,user_id,workspace_id,task_id)
        VALUES (NEW.id,NEW.user_id,NEW.workspace_id,NEW.task_id);
END;
CREATE TRIGGER task_timer_legacy_tracking_close AFTER UPDATE ON time_entries
WHEN OLD.ended_at IS NULL AND NEW.ended_at IS NOT NULL BEGIN
    DELETE FROM task_timer_legacy_open WHERE time_entry_id=NEW.id;
END;

-- PG051: owner-private references, scoped relations and separate sealed key.
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
CREATE TRIGGER zotero_reference_document_purge BEFORE DELETE ON documents BEGIN
    UPDATE zotero_references SET document_id=NULL WHERE workspace_id=OLD.workspace_id AND document_id=OLD.id;
END;
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
-- XOR makes these partial unique indexes the exact NULLS NOT DISTINCT key.
CREATE UNIQUE INDEX zotero_links_document_unique ON zotero_links(workspace_id,reference_id,document_id,anchor) WHERE document_id IS NOT NULL;
CREATE UNIQUE INDEX zotero_links_task_unique ON zotero_links(workspace_id,reference_id,task_id,anchor) WHERE task_id IS NOT NULL;

-- PG030 cascade-safe journaling. These ONLY internal cleanup IDs are opaque
-- random BLOB16 under the coordinator's narrow allocation decision; they are
-- not public UUIDv7s and make no UUID version/variant claim. Parent FKs retain
-- their original NO ACTION behavior. Every actual attachment DELETE journals.
CREATE TRIGGER attachment_object_cleanups_after_attachment_delete AFTER DELETE ON attachments BEGIN
    INSERT INTO attachment_object_cleanups(id,workspace_id,attachment_id,storage_key)
        VALUES (randomblob(16),OLD.workspace_id,OLD.id,OLD.storage_key);
    INSERT INTO attachment_object_cleanups(id,workspace_id,attachment_id,storage_key)
        SELECT randomblob(16),OLD.workspace_id,OLD.id,json_extract(OLD.variants,'$.preview.key')
        WHERE json_type(OLD.variants,'$.preview.key')='text';
END;
-- PG028 automatic project/task collection materialization is a named Rust
-- operation with app UUIDv7s, in the same transaction (create/clone/import/
-- recurrence). Its absence is required pending consumer work, not support.

-- Concrete coordinator/W1 adaptation of PG001 events_seq, not a PG marker.
-- The named emitter increments once per VISIBLE event in its writer tx using
-- UPDATE ... WHERE last_seq<9223372036854775807 RETURNING last_seq. Deferred
-- import rows allocate only upon publication. Retention never recycles numbers.
CREATE TABLE event_sequence (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id=1),
    last_seq INTEGER NOT NULL CHECK (typeof(last_seq)='integer' AND last_seq>=0)
) STRICT;
INSERT INTO event_sequence(id,last_seq) VALUES (1,0);
CREATE TRIGGER event_sequence_no_delete BEFORE DELETE ON event_sequence BEGIN
    SELECT RAISE(ABORT,'event sequence cannot be deleted');
END;
CREATE TRIGGER event_sequence_no_reset BEFORE UPDATE ON event_sequence
WHEN NEW.id<>OLD.id OR NEW.last_seq<OLD.last_seq BEGIN
    SELECT RAISE(ABORT,'event sequence cannot be reset');
END;
CREATE TRIGGER event_sequence_no_replace BEFORE INSERT ON event_sequence
WHEN EXISTS (SELECT 1 FROM event_sequence) BEGIN
    SELECT RAISE(ABORT,'event sequence already exists');
END;
