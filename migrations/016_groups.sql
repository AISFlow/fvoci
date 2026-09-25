CREATE TABLE fvoci.groups (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    name text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT groups_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT groups_name_check CHECK (
        char_length(btrim(name)) >= 1 AND char_length(name) <= 100
    )
);

ALTER TABLE fvoci.groups ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.groups FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.groups
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.group_members (
    workspace_id uuid NOT NULL,
    group_id uuid NOT NULL,
    user_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, group_id, user_id),
    CONSTRAINT group_members_workspace_group_fk
        FOREIGN KEY (workspace_id, group_id)
        REFERENCES fvoci.groups (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT group_members_workspace_user_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id)
        ON DELETE CASCADE
);

CREATE INDEX group_members_workspace_id_user_id_idx
    ON fvoci.group_members (workspace_id, user_id);

ALTER TABLE fvoci.group_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.group_members FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.group_members
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

ALTER TABLE fvoci.project_members ADD COLUMN id uuid;
UPDATE fvoci.project_members SET id = uuidv7() WHERE id IS NULL;
ALTER TABLE fvoci.project_members ALTER COLUMN id SET NOT NULL;
ALTER TABLE fvoci.project_members DROP CONSTRAINT project_members_pkey;
ALTER TABLE fvoci.project_members ADD PRIMARY KEY (id);
ALTER TABLE fvoci.project_members
    ADD CONSTRAINT project_members_workspace_id_id_unique UNIQUE (workspace_id, id);
ALTER TABLE fvoci.project_members ALTER COLUMN user_id DROP NOT NULL;
ALTER TABLE fvoci.project_members ADD COLUMN group_id uuid;
ALTER TABLE fvoci.project_members
    ADD CONSTRAINT project_members_principal_xor_check
    CHECK ((user_id IS NULL) <> (group_id IS NULL));
CREATE UNIQUE INDEX project_members_user_unique
    ON fvoci.project_members (workspace_id, project_id, user_id);
CREATE UNIQUE INDEX project_members_group_unique
    ON fvoci.project_members (workspace_id, project_id, group_id);
ALTER TABLE fvoci.project_members
    ADD CONSTRAINT project_members_workspace_group_fk
        FOREIGN KEY (workspace_id, group_id)
        REFERENCES fvoci.groups (workspace_id, id)
        ON DELETE CASCADE;
CREATE INDEX project_members_workspace_id_group_id_idx
    ON fvoci.project_members (workspace_id, group_id)
    WHERE group_id IS NOT NULL;

CREATE TABLE fvoci.document_members (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    document_id uuid NOT NULL,
    user_id uuid,
    group_id uuid,
    role text NOT NULL,
    CONSTRAINT document_members_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT document_members_role_check CHECK (role IN ('lead', 'member', 'viewer')),
    CONSTRAINT document_members_principal_xor_check
        CHECK ((user_id IS NULL) <> (group_id IS NULL)),
    CONSTRAINT document_members_workspace_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT document_members_workspace_user_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id)
        ON DELETE CASCADE,
    CONSTRAINT document_members_workspace_group_fk
        FOREIGN KEY (workspace_id, group_id)
        REFERENCES fvoci.groups (workspace_id, id)
        ON DELETE CASCADE
);

CREATE UNIQUE INDEX document_members_user_unique
    ON fvoci.document_members (workspace_id, document_id, user_id);
CREATE UNIQUE INDEX document_members_group_unique
    ON fvoci.document_members (workspace_id, document_id, group_id);
CREATE INDEX document_members_workspace_id_user_id_idx
    ON fvoci.document_members (workspace_id, user_id);
CREATE INDEX document_members_workspace_id_group_id_idx
    ON fvoci.document_members (workspace_id, group_id)
    WHERE group_id IS NOT NULL;

ALTER TABLE fvoci.document_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.document_members FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.document_members
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));
