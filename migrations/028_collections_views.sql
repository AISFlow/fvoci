-- Document tags, typed collections (fields, options, values, collection views)
-- and per-user project saved views.
-- 025/026/027 are held by open branches (account, admin, integrations); the
-- coordinator renumbers at integration if needed.
--
-- Source: packages/db/src/pg/schema/{documents,collections,tasks}.ts and the
-- collection triggers in packages/db/src/pg/bootstrap/security.sql.

CREATE TABLE fvoci.document_tags (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    name text NOT NULL,
    color text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT document_tags_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT document_tags_name_check CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    CONSTRAINT document_tags_color_check CHECK (
        color IN ('gray', 'red', 'orange', 'amber', 'green', 'teal', 'blue', 'violet', 'pink')
    )
);
CREATE UNIQUE INDEX document_tags_workspace_id_lower_name_idx
    ON fvoci.document_tags (workspace_id, lower(name));

CREATE TABLE fvoci.document_tag_assignments (
    workspace_id uuid NOT NULL,
    document_id uuid NOT NULL,
    tag_id uuid NOT NULL,
    PRIMARY KEY (workspace_id, document_id, tag_id),
    CONSTRAINT document_tag_assignments_workspace_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id) ON DELETE CASCADE,
    CONSTRAINT document_tag_assignments_workspace_tag_fk
        FOREIGN KEY (workspace_id, tag_id)
        REFERENCES fvoci.document_tags (workspace_id, id) ON DELETE CASCADE
);
CREATE INDEX document_tag_assignments_workspace_id_tag_id_idx
    ON fvoci.document_tag_assignments (workspace_id, tag_id);

CREATE TABLE fvoci.collections (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    project_id uuid,
    kind text NOT NULL,
    name text NOT NULL,
    version integer NOT NULL DEFAULT 1,
    deleted_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT collections_workspace_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT collections_kind_check CHECK (
        kind IN ('document', 'task') AND (kind <> 'task' OR project_id IS NOT NULL)
    ),
    CONSTRAINT collections_version_check CHECK (version > 0),
    CONSTRAINT collections_workspace_project_fk
        FOREIGN KEY (workspace_id, project_id)
        REFERENCES fvoci.projects (workspace_id, id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX collections_task_project_unique
    ON fvoci.collections (workspace_id, project_id) WHERE kind = 'task';
CREATE INDEX collections_scope_idx ON fvoci.collections (workspace_id, project_id, kind);

CREATE TABLE fvoci.collection_items (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    collection_id uuid NOT NULL,
    document_id uuid,
    task_id uuid,
    version integer NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT collection_items_collection_id_unique UNIQUE (workspace_id, collection_id, id),
    CONSTRAINT collection_items_target_check CHECK (num_nonnulls(document_id, task_id) = 1),
    CONSTRAINT collection_items_version_check CHECK (version > 0),
    CONSTRAINT collection_items_collection_fk
        FOREIGN KEY (workspace_id, collection_id)
        REFERENCES fvoci.collections (workspace_id, id) ON DELETE CASCADE,
    CONSTRAINT collection_items_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id) ON DELETE CASCADE,
    CONSTRAINT collection_items_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX collection_items_document_unique
    ON fvoci.collection_items (workspace_id, document_id) WHERE document_id IS NOT NULL;
CREATE UNIQUE INDEX collection_items_task_unique
    ON fvoci.collection_items (workspace_id, task_id) WHERE task_id IS NOT NULL;
CREATE INDEX collection_items_collection_idx
    ON fvoci.collection_items (workspace_id, collection_id, id);

CREATE TABLE fvoci.collection_fields (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    collection_id uuid NOT NULL,
    key text NOT NULL,
    name text NOT NULL,
    description text,
    type text NOT NULL,
    sort_key text NOT NULL,
    version integer NOT NULL DEFAULT 1,
    deleted_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT collection_fields_collection_id_unique UNIQUE (workspace_id, collection_id, id),
    CONSTRAINT collection_fields_collection_key_unique UNIQUE (collection_id, key),
    CONSTRAINT collection_fields_type_unique UNIQUE (workspace_id, collection_id, id, type),
    CONSTRAINT collection_fields_key_check CHECK (key ~ '^[a-z][a-z0-9_]*$' AND length(key) <= 50),
    CONSTRAINT collection_fields_type_check CHECK (
        type IN ('text', 'paragraph', 'number', 'date', 'datetime', 'checkbox', 'select',
                 'multi_select', 'checkboxes', 'user', 'user_multi', 'labels')
    ),
    CONSTRAINT collection_fields_version_check CHECK (version > 0),
    CONSTRAINT collection_fields_collection_fk
        FOREIGN KEY (workspace_id, collection_id)
        REFERENCES fvoci.collections (workspace_id, id) ON DELETE CASCADE
);

CREATE TABLE fvoci.collection_options (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    collection_id uuid NOT NULL,
    field_id uuid NOT NULL,
    key text NOT NULL,
    label text NOT NULL,
    sort_key text NOT NULL,
    deleted_at timestamptz,
    CONSTRAINT collection_options_field_id_unique UNIQUE (workspace_id, collection_id, field_id, id),
    CONSTRAINT collection_options_field_key_unique UNIQUE (field_id, key),
    CONSTRAINT collection_options_field_fk
        FOREIGN KEY (workspace_id, collection_id, field_id)
        REFERENCES fvoci.collection_fields (workspace_id, collection_id, id) ON DELETE CASCADE
);

CREATE TABLE fvoci.collection_values (
    workspace_id uuid NOT NULL,
    collection_id uuid NOT NULL,
    item_id uuid NOT NULL,
    field_id uuid NOT NULL,
    field_type text NOT NULL,
    value_text text,
    value_number numeric,
    value_date date,
    value_ts timestamptz,
    value_bool boolean,
    PRIMARY KEY (workspace_id, collection_id, item_id, field_id),
    CONSTRAINT collection_values_item_fk
        FOREIGN KEY (workspace_id, collection_id, item_id)
        REFERENCES fvoci.collection_items (workspace_id, collection_id, id) ON DELETE CASCADE,
    CONSTRAINT collection_values_field_fk
        FOREIGN KEY (workspace_id, collection_id, field_id, field_type)
        REFERENCES fvoci.collection_fields (workspace_id, collection_id, id, type) ON DELETE CASCADE,
    CONSTRAINT collection_values_type_check CHECK (
        num_nonnulls(value_text, value_number, value_date, value_ts, value_bool) = 1 AND (
            (field_type IN ('text', 'paragraph') AND value_text IS NOT NULL) OR
            (field_type = 'number' AND value_number IS NOT NULL
                AND value_number NOT IN ('NaN'::numeric, 'Infinity'::numeric, '-Infinity'::numeric)) OR
            (field_type = 'date' AND value_date IS NOT NULL AND isfinite(value_date)) OR
            (field_type = 'datetime' AND value_ts IS NOT NULL AND isfinite(value_ts)) OR
            (field_type = 'checkbox' AND value_bool IS NOT NULL)
        )
    )
);
CREATE INDEX collection_values_date_idx
    ON fvoci.collection_values (workspace_id, collection_id, field_id, value_date, item_id);
CREATE INDEX collection_values_ts_idx
    ON fvoci.collection_values (workspace_id, collection_id, field_id, value_ts, item_id);
CREATE INDEX collection_values_number_idx
    ON fvoci.collection_values (workspace_id, collection_id, field_id, value_number, item_id);
CREATE INDEX collection_values_item_idx
    ON fvoci.collection_values (workspace_id, item_id);

CREATE TABLE fvoci.collection_choices (
    workspace_id uuid NOT NULL,
    collection_id uuid NOT NULL,
    item_id uuid NOT NULL,
    field_id uuid NOT NULL,
    field_type text NOT NULL,
    option_id uuid NOT NULL,
    PRIMARY KEY (workspace_id, collection_id, item_id, field_id, option_id),
    CONSTRAINT collection_choices_type_check CHECK (
        field_type IN ('select', 'multi_select', 'checkboxes', 'labels')
    ),
    CONSTRAINT collection_choices_item_fk
        FOREIGN KEY (workspace_id, collection_id, item_id)
        REFERENCES fvoci.collection_items (workspace_id, collection_id, id) ON DELETE CASCADE,
    CONSTRAINT collection_choices_field_fk
        FOREIGN KEY (workspace_id, collection_id, field_id, field_type)
        REFERENCES fvoci.collection_fields (workspace_id, collection_id, id, type) ON DELETE CASCADE,
    CONSTRAINT collection_choices_option_fk
        FOREIGN KEY (workspace_id, collection_id, field_id, option_id)
        REFERENCES fvoci.collection_options (workspace_id, collection_id, field_id, id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX collection_choices_single_unique
    ON fvoci.collection_choices (workspace_id, item_id, field_id) WHERE field_type = 'select';
CREATE INDEX collection_choices_option_idx
    ON fvoci.collection_choices (workspace_id, collection_id, field_id, option_id, item_id);
CREATE INDEX collection_choices_item_idx
    ON fvoci.collection_choices (workspace_id, item_id);

CREATE TABLE fvoci.collection_people (
    workspace_id uuid NOT NULL,
    collection_id uuid NOT NULL,
    item_id uuid NOT NULL,
    field_id uuid NOT NULL,
    field_type text NOT NULL,
    user_id uuid NOT NULL,
    PRIMARY KEY (workspace_id, collection_id, item_id, field_id, user_id),
    CONSTRAINT collection_people_type_check CHECK (field_type IN ('user', 'user_multi')),
    CONSTRAINT collection_people_item_fk
        FOREIGN KEY (workspace_id, collection_id, item_id)
        REFERENCES fvoci.collection_items (workspace_id, collection_id, id) ON DELETE CASCADE,
    CONSTRAINT collection_people_field_fk
        FOREIGN KEY (workspace_id, collection_id, field_id, field_type)
        REFERENCES fvoci.collection_fields (workspace_id, collection_id, id, type) ON DELETE CASCADE,
    CONSTRAINT collection_people_member_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX collection_people_single_unique
    ON fvoci.collection_people (workspace_id, item_id, field_id) WHERE field_type = 'user';
CREATE INDEX collection_people_user_idx
    ON fvoci.collection_people (workspace_id, user_id, collection_id, field_id, item_id);
CREATE INDEX collection_people_item_idx
    ON fvoci.collection_people (workspace_id, item_id);

CREATE TABLE fvoci.collection_views (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    collection_id uuid NOT NULL,
    owner_id uuid NOT NULL,
    visibility text NOT NULL,
    name text NOT NULL,
    type text NOT NULL,
    config jsonb NOT NULL DEFAULT '{}'::jsonb,
    version integer NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT collection_views_visibility_check CHECK (visibility IN ('private', 'shared')),
    CONSTRAINT collection_views_type_check CHECK (type IN ('table', 'board', 'calendar')),
    CONSTRAINT collection_views_version_check CHECK (version > 0),
    CONSTRAINT collection_views_config_size_check CHECK (octet_length(config::text) <= 262144),
    CONSTRAINT collection_views_collection_fk
        FOREIGN KEY (workspace_id, collection_id)
        REFERENCES fvoci.collections (workspace_id, id) ON DELETE CASCADE,
    CONSTRAINT collection_views_owner_fk
        FOREIGN KEY (workspace_id, owner_id)
        REFERENCES fvoci.memberships (workspace_id, user_id) ON DELETE CASCADE
);
CREATE INDEX collection_views_collection_idx
    ON fvoci.collection_views (workspace_id, collection_id, owner_id);

-- Project saved views (source table `views`): private to their owner.
CREATE TABLE fvoci.views (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    project_id uuid NOT NULL,
    user_id uuid NOT NULL,
    name text NOT NULL,
    type text NOT NULL,
    config jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT views_type_check CHECK (type IN ('list', 'board', 'calendar', 'gantt', 'table')),
    CONSTRAINT views_config_size_check CHECK (octet_length(config::text) <= 262144),
    CONSTRAINT views_workspace_project_fk
        FOREIGN KEY (workspace_id, project_id)
        REFERENCES fvoci.projects (workspace_id, id) ON DELETE CASCADE,
    CONSTRAINT views_workspace_user_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id) ON DELETE CASCADE
);
CREATE INDEX views_workspace_id_project_id_user_id_idx
    ON fvoci.views (workspace_id, project_id, user_id);
CREATE INDEX views_workspace_id_user_id_project_id_calendar_idx
    ON fvoci.views (workspace_id, user_id, project_id) WHERE type = 'calendar';

-- Items must stay inside their collection's kind and project scope.
CREATE FUNCTION fvoci.check_collection_item_scope()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    c_kind text;
    c_workspace uuid;
    c_project uuid;
    t_workspace uuid;
    t_project uuid;
BEGIN
    SELECT kind, workspace_id, project_id INTO STRICT c_kind, c_workspace, c_project
    FROM fvoci.collections WHERE id = NEW.collection_id FOR SHARE;
    IF NEW.document_id IS NOT NULL THEN
        SELECT workspace_id, project_id INTO STRICT t_workspace, t_project
        FROM fvoci.documents WHERE id = NEW.document_id FOR SHARE;
        IF c_kind <> 'document' THEN
            RAISE EXCEPTION 'collection kind mismatch' USING ERRCODE = '23514';
        END IF;
    ELSE
        SELECT workspace_id, project_id INTO STRICT t_workspace, t_project
        FROM fvoci.tasks WHERE id = NEW.task_id FOR SHARE;
        IF c_kind <> 'task' THEN
            RAISE EXCEPTION 'collection kind mismatch' USING ERRCODE = '23514';
        END IF;
    END IF;
    IF c_workspace <> NEW.workspace_id OR t_workspace <> NEW.workspace_id
        OR c_project IS DISTINCT FROM t_project THEN
        RAISE EXCEPTION 'collection scope mismatch' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER collection_items_scope
    BEFORE INSERT OR UPDATE OF collection_id, document_id, task_id, workspace_id
    ON fvoci.collection_items
    FOR EACH ROW EXECUTE FUNCTION fvoci.check_collection_item_scope();

CREATE FUNCTION fvoci.keep_collection_scope()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS NOT DISTINCT FROM OLD.project_id AND NEW.workspace_id = OLD.workspace_id THEN
        IF TG_TABLE_NAME <> 'collections' THEN RETURN NEW; END IF;
        IF NEW.kind = OLD.kind THEN RETURN NEW; END IF;
    END IF;
    IF TG_TABLE_NAME = 'collections' THEN
        IF EXISTS (SELECT 1 FROM fvoci.collection_items WHERE collection_id = OLD.id) THEN
            RAISE EXCEPTION 'populated collection scope is immutable' USING ERRCODE = '23514';
        END IF;
    ELSIF TG_TABLE_NAME = 'documents' THEN
        IF EXISTS (SELECT 1 FROM fvoci.collection_items WHERE document_id = OLD.id) THEN
            RAISE EXCEPTION 'detach document before changing scope' USING ERRCODE = '23514';
        END IF;
    ELSE
        IF EXISTS (SELECT 1 FROM fvoci.collection_items WHERE task_id = OLD.id) THEN
            RAISE EXCEPTION 'detach task before changing scope' USING ERRCODE = '23514';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER collections_keep_scope
    BEFORE UPDATE OF workspace_id, project_id, kind ON fvoci.collections
    FOR EACH ROW EXECUTE FUNCTION fvoci.keep_collection_scope();
CREATE TRIGGER documents_keep_collection_scope
    BEFORE UPDATE OF workspace_id, project_id ON fvoci.documents
    FOR EACH ROW EXECUTE FUNCTION fvoci.keep_collection_scope();
CREATE TRIGGER tasks_keep_collection_scope
    BEFORE UPDATE OF workspace_id, project_id ON fvoci.tasks
    FOR EACH ROW EXECUTE FUNCTION fvoci.keep_collection_scope();

CREATE FUNCTION fvoci.keep_collection_field_identity()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF (NEW.workspace_id, NEW.collection_id, NEW.key, NEW.type)
        IS DISTINCT FROM (OLD.workspace_id, OLD.collection_id, OLD.key, OLD.type) THEN
        RAISE EXCEPTION 'field identity is immutable' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER collection_fields_identity
    BEFORE UPDATE ON fvoci.collection_fields
    FOR EACH ROW EXECUTE FUNCTION fvoci.keep_collection_field_identity();

-- Every project owns exactly one task collection and every task is an item of
-- it (source projects.create / tasks.create). Triggers cover every insert path
-- (create, clone, recurrence, import) in the inserting transaction; they run as
-- the invoking role under the caller's tenant context.
CREATE FUNCTION fvoci.create_project_task_collection()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO fvoci.collections (id, workspace_id, project_id, kind, name)
    VALUES (gen_random_uuid(), NEW.workspace_id, NEW.id, 'task', NEW.name);
    RETURN NEW;
END;
$$;

CREATE TRIGGER projects_task_collection
    AFTER INSERT ON fvoci.projects
    FOR EACH ROW EXECUTE FUNCTION fvoci.create_project_task_collection();

CREATE FUNCTION fvoci.attach_task_collection_item()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    target uuid;
BEGIN
    SELECT id INTO target FROM fvoci.collections
    WHERE workspace_id = NEW.workspace_id AND project_id = NEW.project_id AND kind = 'task';
    IF target IS NULL THEN
        RAISE EXCEPTION 'task collection missing' USING ERRCODE = '23514';
    END IF;
    INSERT INTO fvoci.collection_items (id, workspace_id, collection_id, task_id)
    VALUES (gen_random_uuid(), NEW.workspace_id, target, NEW.id);
    RETURN NEW;
END;
$$;

CREATE TRIGGER tasks_collection_item
    AFTER INSERT ON fvoci.tasks
    FOR EACH ROW EXECUTE FUNCTION fvoci.attach_task_collection_item();

-- Backfill existing projects and tasks. projects/tasks are FORCE RLS with a
-- tenant-only policy, which hides every row from a migration owner that is
-- not a superuser (or BYPASSRLS). Lift FORCE for the backfill only; the
-- owner then reads all rows, and FORCE is restored in the same transaction.
ALTER TABLE fvoci.projects NO FORCE ROW LEVEL SECURITY;
ALTER TABLE fvoci.tasks NO FORCE ROW LEVEL SECURITY;

INSERT INTO fvoci.collections (id, workspace_id, project_id, kind, name, created_at, updated_at)
SELECT gen_random_uuid(), p.workspace_id, p.id, 'task', p.name, p.created_at, p.created_at
FROM fvoci.projects p;

INSERT INTO fvoci.collection_items (id, workspace_id, collection_id, task_id, created_at, updated_at)
SELECT gen_random_uuid(), t.workspace_id, c.id, t.id, t.created_at, t.created_at
FROM fvoci.tasks t
INNER JOIN fvoci.collections c
    ON c.workspace_id = t.workspace_id AND c.project_id = t.project_id AND c.kind = 'task';

ALTER TABLE fvoci.projects FORCE ROW LEVEL SECURITY;
ALTER TABLE fvoci.tasks FORCE ROW LEVEL SECURITY;

ALTER TABLE fvoci.document_tags ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.document_tags FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.document_tags
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

ALTER TABLE fvoci.document_tag_assignments ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.document_tag_assignments FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.document_tag_assignments
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

ALTER TABLE fvoci.collections ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.collections FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.collections
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

ALTER TABLE fvoci.collection_items ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.collection_items FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.collection_items
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

ALTER TABLE fvoci.collection_fields ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.collection_fields FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.collection_fields
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

ALTER TABLE fvoci.collection_options ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.collection_options FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.collection_options
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

ALTER TABLE fvoci.collection_values ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.collection_values FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.collection_values
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

ALTER TABLE fvoci.collection_choices ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.collection_choices FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.collection_choices
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

ALTER TABLE fvoci.collection_people ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.collection_people FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.collection_people
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

ALTER TABLE fvoci.collection_views ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.collection_views FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.collection_views
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

ALTER TABLE fvoci.views ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.views FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.views
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));
