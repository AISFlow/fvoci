-- FVOCI SQLite-family new-install baseline, lineage fvoci-sqlite-060, step 08: collections.
-- typed collections, saved views and their scope triggers (project/task collection materialization is a named Rust operation).

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
CREATE UNIQUE INDEX collections_task_project_unique ON collections(workspace_id,project_id) WHERE kind='task';
CREATE INDEX collections_scope_idx ON collections(workspace_id,project_id,kind);
CREATE UNIQUE INDEX collection_items_document_unique ON collection_items(workspace_id,document_id) WHERE document_id IS NOT NULL;
CREATE UNIQUE INDEX collection_items_task_unique ON collection_items(workspace_id,task_id) WHERE task_id IS NOT NULL;
CREATE INDEX collection_items_collection_idx ON collection_items(workspace_id,collection_id,id);
CREATE INDEX collection_values_date_idx ON collection_values(workspace_id,collection_id,field_id,value_date,item_id);
CREATE INDEX collection_values_ts_idx ON collection_values(workspace_id,collection_id,field_id,value_ts,item_id);
CREATE INDEX collection_values_number_idx ON collection_values(workspace_id,collection_id,field_id,value_number,item_id);
CREATE INDEX collection_values_item_idx ON collection_values(workspace_id,item_id);
CREATE UNIQUE INDEX collection_choices_single_unique ON collection_choices(workspace_id,item_id,field_id) WHERE field_type='select';
CREATE INDEX collection_choices_option_idx ON collection_choices(workspace_id,collection_id,field_id,option_id,item_id);
CREATE INDEX collection_choices_item_idx ON collection_choices(workspace_id,item_id);
CREATE UNIQUE INDEX collection_people_single_unique ON collection_people(workspace_id,item_id,field_id) WHERE field_type='user';
CREATE INDEX collection_people_user_idx ON collection_people(workspace_id,user_id,collection_id,field_id,item_id);
CREATE INDEX collection_people_item_idx ON collection_people(workspace_id,item_id);
CREATE INDEX collection_views_collection_idx ON collection_views(workspace_id,collection_id,owner_id);
CREATE INDEX views_workspace_id_project_id_user_id_idx ON views(workspace_id,project_id,user_id);
CREATE INDEX views_workspace_id_user_id_project_id_calendar_idx ON views(workspace_id,user_id,project_id) WHERE type='calendar';
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
