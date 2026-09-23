ALTER TABLE fvoci.workspaces
    ADD COLUMN next_document_number integer NOT NULL DEFAULT 0;

CREATE TABLE fvoci.documents (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    title text NOT NULL,
    icon text,
    path text NOT NULL,
    parent_id uuid,
    sort_key text NOT NULL,
    project_id uuid,
    number integer NOT NULL,
    status text NOT NULL,
    schema_version integer NOT NULL,
    text text NOT NULL DEFAULT '',
    chosung text NOT NULL DEFAULT '',
    version integer NOT NULL DEFAULT 1,
    created_by uuid NOT NULL REFERENCES fvoci.users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    content_json jsonb NOT NULL,
    kind text NOT NULL DEFAULT 'doc',
    CONSTRAINT documents_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT documents_workspace_id_project_id_number_unique
        UNIQUE NULLS NOT DISTINCT (workspace_id, project_id, number),
    CONSTRAINT documents_path_check CHECK (
        path COLLATE "C" ~ '^[0-9a-f]{32}(\.[0-9a-f]{32}){0,19}$'
    ),
    CONSTRAINT documents_status_check CHECK (status IN ('draft', 'published', 'archived')),
    CONSTRAINT documents_kind_check CHECK (kind IN ('doc', 'wiki', 'template')),
    CONSTRAINT documents_workspace_parent_fk
        FOREIGN KEY (workspace_id, parent_id)
        REFERENCES fvoci.documents (workspace_id, id)
);

CREATE INDEX documents_created_by_idx ON fvoci.documents (created_by);
CREATE INDEX documents_workspace_id_parent_id_idx
    ON fvoci.documents (workspace_id, parent_id);
CREATE INDEX documents_workspace_id_project_id_idx
    ON fvoci.documents (workspace_id, project_id);
CREATE INDEX documents_workspace_id_updated_at_id_idx
    ON fvoci.documents (workspace_id, updated_at, id)
    WHERE deleted_at IS NULL;
CREATE INDEX documents_workspace_id_updated_at_live_idx
    ON fvoci.documents (workspace_id, updated_at DESC, id DESC)
    WHERE deleted_at IS NULL;

ALTER TABLE fvoci.documents ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.documents
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.document_states (
    workspace_id uuid NOT NULL,
    document_id uuid NOT NULL,
    state bytea NOT NULL,
    encoding smallint NOT NULL DEFAULT 1,
    compacted_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT document_states_pkey PRIMARY KEY (workspace_id, document_id),
    CONSTRAINT document_states_encoding_check CHECK (encoding = 1),
    CONSTRAINT document_states_workspace_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE CASCADE
);

ALTER TABLE fvoci.document_states ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.document_states
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()));
