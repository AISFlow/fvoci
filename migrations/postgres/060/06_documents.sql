-- FVOCI PostgreSQL baseline fvoci-postgres-060, step 06: documents.
-- documents and their collaboration rooms, document principals, revisions
-- (documents and tasks), comments, stars, share links, document tags, templates,
-- task origins and the OFF (versioned save) command receipts.

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
        REFERENCES fvoci.documents (workspace_id, id),
    CONSTRAINT documents_project_fk
        FOREIGN KEY (workspace_id, project_id)
        REFERENCES fvoci.projects (workspace_id, id)
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
    writer_generation bigint NOT NULL DEFAULT 0,
    snapshot_cutoff_seq bigint NOT NULL DEFAULT 0,
    tail_seq bigint NOT NULL DEFAULT 0,
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

CREATE TABLE fvoci.document_collab_updates (
    workspace_id uuid NOT NULL,
    document_id uuid NOT NULL,
    seq bigint NOT NULL,
    op_id uuid NOT NULL,
    payload bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT document_collab_updates_pkey PRIMARY KEY (workspace_id, document_id, seq),
    CONSTRAINT document_collab_updates_op_unique UNIQUE (workspace_id, document_id, op_id),
    CONSTRAINT document_collab_updates_workspace_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT document_collab_updates_payload_len_check CHECK (
        octet_length(payload) >= 1 AND octet_length(payload) <= 8388608
    )
);

CREATE INDEX document_collab_updates_tail_idx
    ON fvoci.document_collab_updates (workspace_id, document_id, seq);

CREATE TABLE fvoci.document_collab_op_receipts (
    workspace_id uuid NOT NULL,
    document_id uuid NOT NULL,
    op_id uuid NOT NULL,
    seq bigint NOT NULL,
    payload_len bigint NOT NULL,
    payload_sha256 bytea NOT NULL,
    actor_user_id uuid NOT NULL REFERENCES fvoci.users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT document_collab_op_receipts_pkey PRIMARY KEY (workspace_id, document_id, op_id),
    CONSTRAINT document_collab_op_receipts_workspace_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT document_collab_op_receipts_payload_len_check CHECK (
        payload_len >= 1 AND payload_len <= 8388608
    ),
    CONSTRAINT document_collab_op_receipts_sha256_len_check CHECK (
        octet_length(payload_sha256) = 32
    )
);

CREATE INDEX document_collab_op_receipts_lookup_idx
    ON fvoci.document_collab_op_receipts (workspace_id, document_id, seq);

ALTER TABLE fvoci.document_collab_op_receipts ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.document_collab_op_receipts
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()));

ALTER TABLE fvoci.document_collab_updates ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.document_collab_updates
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()));

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

-- Document and task revisions share the table. Restore appends to the existing
-- history with its provenance columns; source IDs remain provenance even if a
-- retention policy later removes an automatic source revision, so there is
-- deliberately no cascade FK on restored_from_id.
CREATE TABLE fvoci.revisions (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    target_kind text NOT NULL,
    target_id uuid NOT NULL,
    y_snapshot bytea NOT NULL,
    encoding smallint NOT NULL DEFAULT 1,
    content_json jsonb NOT NULL,
    text text NOT NULL,
    reason text NOT NULL,
    created_by uuid REFERENCES fvoci.users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    restored_from_id uuid,
    restore_correlation_id uuid,
    restore_base_tail_seq bigint,
    restore_committed_tail_seq bigint,
    CONSTRAINT revisions_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT revisions_target_kind_check CHECK (target_kind IN ('document', 'task')),
    CONSTRAINT revisions_reason_check
        CHECK (reason IN ('manual', 'session', 'scheduled', 'restore')),
    CONSTRAINT revisions_encoding_check CHECK (encoding = 1),
    CONSTRAINT revisions_restore_metadata_check CHECK (
        (reason = 'restore'
            AND created_by IS NOT NULL
            AND restored_from_id IS NOT NULL
            AND restore_correlation_id IS NOT NULL
            AND restore_base_tail_seq IS NOT NULL
            AND restore_base_tail_seq >= 0
            AND restore_committed_tail_seq IS NOT NULL
            AND restore_committed_tail_seq > restore_base_tail_seq
            AND restore_committed_tail_seq - restore_base_tail_seq = 1)
        OR
        (reason <> 'restore'
            AND restored_from_id IS NULL
            AND restore_correlation_id IS NULL
            AND restore_base_tail_seq IS NULL
            AND restore_committed_tail_seq IS NULL)
    )
);

CREATE INDEX revisions_workspace_id_target_id_created_at_id_idx
    ON fvoci.revisions (workspace_id, target_id, created_at DESC, id DESC);

-- A correlation is a single workspace operation. Reuse for a different
-- actor/target/source/base must conflict, including after response loss.
CREATE UNIQUE INDEX revisions_workspace_restore_correlation_idx
    ON fvoci.revisions (workspace_id, restore_correlation_id)
    WHERE restore_correlation_id IS NOT NULL;

ALTER TABLE fvoci.revisions ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.revisions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.revisions
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.comments (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    document_id uuid,
    task_id uuid,
    parent_id uuid,
    created_by uuid NOT NULL REFERENCES fvoci.users (id),
    body text NOT NULL,
    chosung text NOT NULL DEFAULT '',
    resolved_at timestamptz,
    reactions jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT comments_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT comments_parent_xor_check CHECK (
        (document_id IS NULL) <> (task_id IS NULL)
    ),
    CONSTRAINT comments_body_check CHECK (
        char_length(btrim(body)) >= 1 AND char_length(body) <= 8000
    ),
    CONSTRAINT comments_workspace_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT comments_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT comments_workspace_parent_fk
        FOREIGN KEY (workspace_id, parent_id)
        REFERENCES fvoci.comments (workspace_id, id)
        ON DELETE CASCADE
);

CREATE INDEX comments_workspace_id_parent_id_idx
    ON fvoci.comments (workspace_id, parent_id);

CREATE INDEX comments_workspace_id_created_by_idx
    ON fvoci.comments (workspace_id, created_by, created_at, id);

CREATE INDEX comments_workspace_id_document_id_created_at_idx
    ON fvoci.comments (workspace_id, document_id, created_at, id);

CREATE INDEX comments_workspace_id_task_id_created_at_idx
    ON fvoci.comments (workspace_id, task_id, created_at, id);

ALTER TABLE fvoci.comments ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.comments FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.comments
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

-- Stars (per-user favourites).
CREATE TABLE fvoci.stars (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    user_id uuid NOT NULL,
    document_id uuid,
    task_id uuid,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT stars_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT stars_parent_xor_check CHECK ((document_id IS NULL) <> (task_id IS NULL)),
    CONSTRAINT stars_workspace_user_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id)
        ON DELETE CASCADE,
    CONSTRAINT stars_workspace_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT stars_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE
);

CREATE UNIQUE INDEX stars_user_document_unique
    ON fvoci.stars (workspace_id, user_id, document_id);
CREATE UNIQUE INDEX stars_user_task_unique
    ON fvoci.stars (workspace_id, user_id, task_id);
CREATE INDEX stars_workspace_id_document_id_idx ON fvoci.stars (workspace_id, document_id);
CREATE INDEX stars_workspace_id_task_id_idx ON fvoci.stars (workspace_id, task_id);

ALTER TABLE fvoci.stars ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.stars FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.stars
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

-- Public share links. token_hash is SHA-256 hex of a 256-bit random token; the
-- raw token is never stored. The app role cannot SELECT token_hash (column grant
-- in grant-app-role.sql); public resolution goes through the definer function
-- below, which also requires the system context.
CREATE TABLE fvoci.share_links (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    user_id uuid NOT NULL,
    token_hash text NOT NULL,
    document_id uuid,
    project_id uuid,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT share_links_token_hash_unique UNIQUE (token_hash),
    CONSTRAINT share_links_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT share_links_target_xor_check CHECK ((document_id IS NULL) <> (project_id IS NULL)),
    CONSTRAINT share_links_token_hash_format_check CHECK (token_hash ~ '^[0-9a-f]{64}$'),
    CONSTRAINT share_links_workspace_user_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id)
        ON DELETE CASCADE,
    CONSTRAINT share_links_workspace_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT share_links_workspace_project_fk
        FOREIGN KEY (workspace_id, project_id)
        REFERENCES fvoci.projects (workspace_id, id)
        ON DELETE CASCADE
);

CREATE INDEX share_links_workspace_id_document_id_idx
    ON fvoci.share_links (workspace_id, document_id);
CREATE INDEX share_links_workspace_id_project_id_idx
    ON fvoci.share_links (workspace_id, project_id);
CREATE INDEX share_links_workspace_id_user_id_idx
    ON fvoci.share_links (workspace_id, user_id);

ALTER TABLE fvoci.share_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.share_links FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.share_links
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));
-- The system context only ever reads (token lookup); it can never delete or
-- write another tenant's links.
CREATE POLICY system_token_lookup ON fvoci.share_links
    AS PERMISSIVE FOR SELECT TO public
    USING ((SELECT public.app_system_ctx_on()));

-- Public token resolution: exact hash match, unexpired only. Returns the
-- stored hash so the caller can re-compare in constant time; no other secret
-- leaves the table. The caller must enable the system context for the
-- transaction; the predicate is explicit so it holds even when the function
-- owner bypasses RLS.
CREATE FUNCTION fvoci.app_share_link_by_token_hash(p_hash text)
RETURNS TABLE (
    id uuid,
    workspace_id uuid,
    token_hash text,
    document_id uuid,
    project_id uuid,
    expires_at timestamptz
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT s.id, s.workspace_id, s.token_hash, s.document_id, s.project_id, s.expires_at
    FROM fvoci.share_links s
    WHERE s.token_hash = p_hash
      AND s.expires_at > now()
      AND (SELECT public.app_system_ctx_on())
$$;

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

-- Workspace document/task templates.
CREATE TABLE fvoci.templates (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    kind text NOT NULL,
    title text NOT NULL,
    payload jsonb NOT NULL,
    created_by uuid NOT NULL REFERENCES fvoci.users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT templates_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT templates_kind_check CHECK (kind IN ('document', 'task')),
    CONSTRAINT templates_title_check CHECK (length(btrim(title)) BETWEEN 1 AND 200)
);

CREATE INDEX templates_workspace_id_idx ON fvoci.templates (workspace_id);
CREATE INDEX templates_created_by_idx ON fvoci.templates (created_by);

ALTER TABLE fvoci.templates ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.templates FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.templates
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    )
    WITH CHECK (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

-- The task created from a document block (POST documents/:id/tasks).
CREATE TABLE fvoci.task_origins (
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    document_id uuid NOT NULL,
    request_id uuid NOT NULL,
    request_hash text NOT NULL,
    anchor text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT task_origins_workspace_id_task_id_pk PRIMARY KEY (workspace_id, task_id),
    CONSTRAINT task_origins_workspace_document_request_unique
        UNIQUE (workspace_id, document_id, request_id),
    CONSTRAINT task_origins_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT task_origins_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT task_origins_anchor_length_check
        CHECK (anchor IS NULL OR char_length(anchor) <= 200)
);

CREATE INDEX task_origins_document_task_idx
    ON fvoci.task_origins (workspace_id, document_id, task_id);

ALTER TABLE fvoci.task_origins ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.task_origins FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.task_origins
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

-- An ordinary wiki create is one command, including a retry after a lost
-- response. Purge clears only the live target, retaining the original binding
-- and response; a retired command can never create a replacement document.
CREATE TABLE fvoci.wiki_create_commands (
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    command_id uuid NOT NULL,
    actor_user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    request_hash text NOT NULL CHECK (request_hash ~ '^[0-9a-f]{64}$'),
    document_id uuid,
    result_json jsonb NOT NULL CHECK (jsonb_typeof(result_json) = 'object'),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, command_id),
    FOREIGN KEY (workspace_id, document_id) REFERENCES fvoci.documents (workspace_id, id)
        ON DELETE SET NULL (document_id)
);
ALTER TABLE fvoci.wiki_create_commands ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.wiki_create_commands FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.wiki_create_commands
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

-- One OFF body save is an immutable command binding. Live FKs are cleared on
-- purge; the original target/result identity survives and cannot be reused.
CREATE TABLE fvoci.body_save_commands (
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces(id) ON DELETE CASCADE,
    command_id uuid NOT NULL,
    actor_user_id uuid NOT NULL REFERENCES fvoci.users(id) ON DELETE CASCADE,
    credential_id uuid NOT NULL,
    target_kind text NOT NULL CHECK (target_kind IN ('document','task')),
    target_id uuid NOT NULL,
    document_id uuid,
    task_id uuid,
    expected_tail_seq bigint NOT NULL CHECK (expected_tail_seq >= 0 AND expected_tail_seq < 9223372036854775807),
    committed_tail_seq bigint NOT NULL CHECK (committed_tail_seq = expected_tail_seq + 1),
    request_hash text NOT NULL CHECK (request_hash ~ '^[0-9a-f]{64}$'),
    payload_hash bytea NOT NULL CHECK (octet_length(payload_hash)=32),
    result_json jsonb NOT NULL CHECK (jsonb_typeof(result_json)='object'),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id,command_id),
    CHECK ((target_kind='document' AND task_id IS NULL AND (document_id IS NULL OR document_id=target_id))
        OR (target_kind='task' AND document_id IS NULL AND (task_id IS NULL OR task_id=target_id))),
    FOREIGN KEY (workspace_id,document_id) REFERENCES fvoci.documents(workspace_id,id) ON DELETE SET NULL(document_id),
    FOREIGN KEY (workspace_id,task_id) REFERENCES fvoci.tasks(workspace_id,id) ON DELETE SET NULL(task_id)
);
ALTER TABLE fvoci.body_save_commands ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.body_save_commands FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.body_save_commands
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id=(SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id=(SELECT public.app_tenant_id()));

REVOKE EXECUTE ON FUNCTION fvoci.app_share_link_by_token_hash(text) FROM PUBLIC;
