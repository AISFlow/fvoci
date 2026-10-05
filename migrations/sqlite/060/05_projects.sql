-- FVOCI SQLite-family new-install baseline, lineage fvoci-sqlite-060, step 05: projects.
-- projects and principals, workflows/statuses, tasks and relations, activity, time entries, task collaboration rooms and the actor-private stopwatch.

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
CREATE TABLE task_assignees (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    PRIMARY KEY (task_id,user_id),
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE
) STRICT;
CREATE TABLE task_labels (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    label_id BLOB NOT NULL CHECK (typeof(label_id)='blob' AND length(label_id)=16),
    PRIMARY KEY (task_id,label_id),
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,label_id) REFERENCES labels(workspace_id,id) ON DELETE CASCADE
) STRICT;
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
CREATE TABLE task_timer_legacy_open (
    time_entry_id BLOB PRIMARY KEY NOT NULL CHECK (typeof(time_entry_id)='blob' AND length(time_entry_id)=16),
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    FOREIGN KEY (workspace_id,time_entry_id) REFERENCES time_entries(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE
) STRICT;
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
CREATE INDEX projects_workspace_id_deleted_at_idx ON projects(workspace_id,deleted_at);
CREATE INDEX statuses_workspace_project_idx ON statuses(workspace_id,project_id,sort_key COLLATE BINARY);
CREATE INDEX tasks_workspace_project_idx ON tasks(workspace_id,project_id) WHERE deleted_at IS NULL;
CREATE INDEX tasks_workspace_id_milestone_id_idx ON tasks(workspace_id,milestone_id);
CREATE UNIQUE INDEX project_members_user_unique ON project_members(workspace_id,project_id,user_id);
CREATE UNIQUE INDEX project_members_group_unique ON project_members(workspace_id,project_id,group_id);
CREATE INDEX project_members_workspace_user_idx ON project_members(workspace_id,user_id);
CREATE INDEX project_members_workspace_id_group_id_idx ON project_members(workspace_id,group_id) WHERE group_id IS NOT NULL;
CREATE INDEX labels_workspace_id_project_id_idx ON labels(workspace_id,project_id);
CREATE INDEX task_assignees_user_id_idx ON task_assignees(user_id);
CREATE INDEX task_assignees_workspace_id_task_id_idx ON task_assignees(workspace_id,task_id);
CREATE INDEX task_assignees_workspace_id_user_id_idx ON task_assignees(workspace_id,user_id);
CREATE INDEX task_labels_workspace_id_label_id_idx ON task_labels(workspace_id,label_id);
CREATE INDEX task_labels_workspace_id_task_id_idx ON task_labels(workspace_id,task_id);
CREATE INDEX milestones_workspace_id_project_id_idx ON milestones(workspace_id,project_id);
CREATE INDEX task_dependencies_workspace_id_blocked_id_idx ON task_dependencies(workspace_id,blocked_id);
CREATE INDEX task_dependencies_workspace_id_blocker_id_idx ON task_dependencies(workspace_id,blocker_id);
CREATE INDEX task_activity_workspace_task_created_idx ON task_activity(workspace_id,task_id,created_at DESC,id DESC);
CREATE INDEX task_activity_actor_idx ON task_activity(actor_user_id);
CREATE INDEX task_collab_op_receipts_lookup_idx ON task_collab_op_receipts(workspace_id,task_id,seq);
CREATE UNIQUE INDEX time_entries_one_open_per_actor ON time_entries(workspace_id,user_id) WHERE ended_at IS NULL;
CREATE INDEX time_entries_workspace_id_task_id_started_at_idx ON time_entries(workspace_id,task_id,started_at);
CREATE UNIQUE INDEX task_timer_one_unfinished_per_person ON task_timer_runs(user_id) WHERE status<>'stopped';
CREATE INDEX task_timer_runs_task ON task_timer_runs(workspace_id,task_id,user_id);
CREATE UNIQUE INDEX task_timer_one_open_segment ON task_timer_segments(run_id) WHERE ended_at IS NULL;
CREATE INDEX task_timer_legacy_person ON task_timer_legacy_open(user_id);
CREATE INDEX task_timer_audit_user_id_idx ON task_timer_audit(user_id);
CREATE TRIGGER timer_segment_time_entry_purge BEFORE DELETE ON time_entries BEGIN
    UPDATE task_timer_segments SET time_entry_id=NULL WHERE workspace_id=OLD.workspace_id AND time_entry_id=OLD.id;
END;
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
