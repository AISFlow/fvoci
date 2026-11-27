-- The schema of a PostgreSQL database. Every table of an organization's rows is behind
-- the wall of row-level security (`org_wall`), enabled and forced; the rows every
-- database starts with are the system organization's, so the statements reach them as
-- the system does. The functions come first, then the tables, those rows, the keys and
-- indexes, the foreign keys, the triggers, and the wall.
--
-- Identifiers are version 7 UUIDs in their canonical text, 36 characters; times are
-- milliseconds since the Unix epoch. The two files of the baseline make the same
-- tables and indexes, and each table has the same columns, but that PostgreSQL's tables
-- of an organization's rows also have the organization (`org_id`), which their keys
-- hold, and that PostgreSQL has two indexes more: `workspace_members_scope_key`, a key
-- with the organization, and `loose_objects_path_idx`, by the place alone, which
-- SQLite's key of that table is. They differ besides in the column types, in
-- PostgreSQL's wall and in SQLite's triggers that hold one organization.
--
-- This is the schema the migrations 1 to 79 of the servers before it built, with their
-- record replaced (`crate::schema::inspect`). A migration that has been released is
-- never edited: a change is a new file with the next number.
SELECT set_config('fvoci.system', 'on', true);

CREATE FUNCTION audit_log_is_kept() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'DELETE' AND (
           NOT EXISTS (SELECT 1 FROM organizations WHERE id = OLD.org_id)
           OR (OLD.workspace_id IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM workspaces WHERE id = OLD.workspace_id))) THEN
        RETURN OLD;
    END IF;
    IF TG_OP = 'UPDATE'
       AND NEW.identity IS NULL
       AND (NEW.id, NEW.org_id, NEW.workspace_id, NEW.actor_id, NEW.actor_kind, NEW.action,
            NEW.target_id, NEW.subject_id, NEW.created_at, NEW.detail)
           IS NOT DISTINCT FROM
           (OLD.id, OLD.org_id, OLD.workspace_id, OLD.actor_id, OLD.actor_kind, OLD.action,
            OLD.target_id, OLD.subject_id, OLD.created_at, OLD.detail)
       AND EXISTS (SELECT 1 FROM users
                   WHERE id IN (OLD.actor_id, OLD.subject_id) AND erase_started_at > 0) THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'a record of the audit log is not changed or removed'
        USING ERRCODE = 'restrict_violation';
END $$;

CREATE FUNCTION category_containers_container_of_its_organization() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    -- A row of no organization is refused by its column, which takes no NULL.
    IF NEW.org_id IS NULL THEN
        RETURN NEW;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM containers WHERE id = NEW.container_id
        AND org_id IN (NEW.org_id, '00000000-0000-7000-8000-00000000a000')
    ) THEN
        RAISE foreign_key_violation USING
            MESSAGE = 'the board of a category is neither of its organization nor a template';
    END IF;
    RETURN NEW;
END $$;

CREATE FUNCTION course_child_of_its_course() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    trashed_raw TEXT;
    is_trashed_course BOOLEAN := false;
BEGIN
    IF EXISTS (
        SELECT 1 FROM containers
        WHERE id = NEW.container_id AND org_id = NEW.org_id AND kind = 'course'
    ) THEN
        RETURN NEW;
    END IF;
    SELECT container::text INTO trashed_raw
    FROM deleted_containers
    WHERE id = NEW.container_id AND org_id = NEW.org_id;
    IF FOUND THEN
        BEGIN
            is_trashed_course := COALESCE((trashed_raw::json) ->> 'kind', 'board') = 'course';
        EXCEPTION WHEN untranslatable_character OR invalid_text_representation THEN
            is_trashed_course := trashed_raw ~ '"kind"\s*:\s*"course"';
        END;
    END IF;
    IF NOT is_trashed_course THEN
        RAISE foreign_key_violation USING
            MESSAGE = 'the course of a course record is not a course of its organization';
    END IF;
    RETURN NEW;
END $$;

CREATE FUNCTION diaries_are_of_a_space() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.kind = 'diary'
       AND NOT EXISTS (SELECT 1 FROM organizations AS o
                       WHERE o.id = NEW.org_id AND o.kind = 'personal') THEN
        RAISE EXCEPTION 'a diary is of the space of an account: no other workspace has one'
            USING ERRCODE = 'restrict_violation';
    END IF;
    RETURN NEW;
END $$;

CREATE FUNCTION diaries_have_no_link() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM containers AS c
               WHERE c.id = NEW.container_id AND c.org_id = NEW.org_id AND c.kind = 'diary') THEN
        RAISE EXCEPTION 'a diary has no link' USING ERRCODE = 'restrict_violation';
    END IF;
    RETURN NEW;
END $$;

CREATE FUNCTION organizations_keep_their_kind() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    RAISE EXCEPTION 'an organization does not change its kind or its owner'
        USING ERRCODE = 'restrict_violation';
END $$;

CREATE FUNCTION spaces_have_no_invitations() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM organizations AS o
               WHERE o.id = NEW.org_id AND o.kind = 'personal') THEN
        RAISE EXCEPTION 'nobody is invited into the space of an account'
            USING ERRCODE = 'restrict_violation';
    END IF;
    RETURN NEW;
END $$;

CREATE FUNCTION spaces_take_their_owner_alone() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM organizations AS o
               WHERE o.id = NEW.org_id AND o.kind = 'personal'
                 AND o.owner_id IS DISTINCT FROM NEW.user_id) THEN
        RAISE EXCEPTION 'the space of an account has its owner in it and nobody else'
            USING ERRCODE = 'restrict_violation';
    END IF;
    RETURN NEW;
END $$;

CREATE FUNCTION uploads_target_of_its_organization() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    -- A row of no organization is refused by its column, which takes no NULL.
    IF NEW.org_id IS NULL THEN
        RETURN NEW;
    END IF;
    IF NEW.purpose = 'import' THEN
        IF NOT EXISTS (
            SELECT 1 FROM workspaces WHERE id = NEW.container_id AND org_id = NEW.org_id
        ) THEN
            RAISE foreign_key_violation USING
                MESSAGE = 'the workspace of an import is not one of its organization';
        END IF;
    ELSIF NOT EXISTS (
        SELECT 1 FROM containers WHERE id = NEW.container_id AND org_id = NEW.org_id
    ) THEN
        RAISE foreign_key_violation USING
            MESSAGE = 'the board of an upload is not one of its organization';
    END IF;
    RETURN NEW;
END $$;

CREATE TABLE api_token_workspaces (
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    token_id uuid NOT NULL,
    workspace_id uuid NOT NULL
);

CREATE TABLE api_tokens (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    name text NOT NULL,
    token_prefix text NOT NULL,
    token_hash text NOT NULL,
    scope text NOT NULL,
    allow_personal_space boolean DEFAULT false NOT NULL,
    notify_self boolean DEFAULT true NOT NULL,
    origin_provider_id uuid,
    origin_provider_revision bigint,
    origin_issuer text,
    origin_subject text,
    last_used_at bigint,
    last_used_address text,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    revoked_at bigint,
    CONSTRAINT api_tokens_check CHECK ((expires_at > created_at)),
    CONSTRAINT api_tokens_check1 CHECK (((origin_provider_id IS NULL) = (origin_provider_revision IS NULL))),
    CONSTRAINT api_tokens_check2 CHECK (((origin_provider_id IS NULL) = (origin_issuer IS NULL))),
    CONSTRAINT api_tokens_name_check CHECK (((char_length(name) >= 1) AND (char_length(name) <= 80))),
    CONSTRAINT api_tokens_scope_check CHECK ((scope = ANY (ARRAY['read'::text, 'write'::text])))
);

CREATE TABLE audit_log (
    id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    workspace_id uuid,
    actor_id uuid,
    action text NOT NULL,
    target_id uuid NOT NULL,
    created_at bigint NOT NULL,
    subject_id uuid,
    actor_kind text DEFAULT 'person'::text NOT NULL,
    detail text,
    identity text,
    CONSTRAINT audit_log_action_check CHECK ((action = ANY (ARRAY['self_added_to_container'::text, 'added_to_container'::text, 'org_admin_joined'::text, 'org_admin_left'::text, 'org_admin_revoked'::text, 'org_admin_granted'::text, 'org_admin_removed'::text, 'org_member_removed'::text, 'workspace_created'::text, 'workspace_renamed'::text, 'workspace_deleted'::text, 'workspace_restored'::text, 'public_links_disabled'::text, 'public_links_enabled'::text, 'container_copied_out'::text, 'copy_out_disabled'::text, 'copy_out_enabled'::text, 'api_tokens_enabled'::text, 'api_tokens_disabled'::text, 'api_token_revoked'::text, 'oidc_provider_configured'::text, 'oidc_provider_removed'::text, 'sso_required'::text, 'sso_released'::text, 'oidc_provider_pending_saved'::text, 'oidc_provider_pending_applied'::text, 'oidc_provider_pending_discarded'::text, 'password_reset_link_made'::text, 'password_set_by_operator'::text, 'oidc_former_provider_kept'::text, 'oidc_former_provider_ended'::text, 'oidc_former_provider_lapsed'::text, 'oidc_identity_bound'::text, 'oidc_identity_carried_over'::text, 'email_verified_by_operator'::text]))),
    CONSTRAINT audit_log_actor_check CHECK ((((actor_kind = 'person'::text) AND (actor_id IS NOT NULL)) OR ((actor_kind = ANY (ARRAY['server_operator'::text, 'server'::text])) AND (actor_id IS NULL)))),
    CONSTRAINT audit_log_actor_kind_check CHECK ((actor_kind = ANY (ARRAY['person'::text, 'server_operator'::text, 'server'::text]))),
    CONSTRAINT audit_log_place_check CHECK (((workspace_id IS NOT NULL) OR (action = ANY (ARRAY['org_admin_granted'::text, 'org_admin_removed'::text, 'org_member_removed'::text, 'workspace_created'::text, 'workspace_renamed'::text, 'workspace_deleted'::text, 'workspace_restored'::text, 'public_links_disabled'::text, 'public_links_enabled'::text, 'container_copied_out'::text, 'copy_out_disabled'::text, 'copy_out_enabled'::text, 'api_tokens_enabled'::text, 'api_tokens_disabled'::text, 'api_token_revoked'::text, 'oidc_provider_configured'::text, 'oidc_provider_removed'::text, 'sso_required'::text, 'sso_released'::text, 'oidc_provider_pending_saved'::text, 'oidc_provider_pending_applied'::text, 'oidc_provider_pending_discarded'::text, 'password_reset_link_made'::text, 'password_set_by_operator'::text, 'oidc_former_provider_kept'::text, 'oidc_former_provider_ended'::text, 'oidc_former_provider_lapsed'::text, 'oidc_identity_bound'::text, 'oidc_identity_carried_over'::text, 'email_verified_by_operator'::text]))))
);

CREATE TABLE blocks (
    id uuid NOT NULL,
    container_id uuid NOT NULL,
    parent_id uuid,
    created_by uuid,
    modified_by uuid,
    type text NOT NULL,
    title text NOT NULL,
    fields json NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    body jsonb,
    body_version bigint,
    body_text text,
    body_tasks_total bigint DEFAULT 0 NOT NULL,
    body_tasks_done bigint DEFAULT 0 NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    number bigint,
    body_first_image text,
    due_at text,
    due_millis bigint,
    is_done boolean DEFAULT false NOT NULL,
    CONSTRAINT blocks_type_check CHECK ((type = ANY (ARRAY['card'::text, 'view'::text, 'comment'::text, 'attachment'::text, 'page'::text, 'entry'::text, 'unit'::text, 'chapter'::text])))
);

CREATE TABLE body_docs (
    block_id uuid NOT NULL,
    state bytea NOT NULL,
    state_seq bigint NOT NULL,
    log_end bigint NOT NULL,
    projected_seq bigint NOT NULL,
    body_digest bytea NOT NULL,
    epoch bigint NOT NULL,
    last_writer uuid,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    agent_token_id uuid,
    agent_token_name text
);

CREATE TABLE body_links (
    source_id uuid NOT NULL,
    target_id uuid NOT NULL,
    container_id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL
);

CREATE TABLE body_updates (
    block_id uuid NOT NULL,
    seq bigint NOT NULL,
    bytes bytea NOT NULL,
    written_by uuid,
    written_at bigint NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    actor_token_id uuid
);

CREATE TABLE body_version_editors (
    block_id uuid NOT NULL,
    version bigint NOT NULL,
    user_id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL
);

CREATE TABLE body_versions (
    block_id uuid NOT NULL,
    version bigint NOT NULL,
    body jsonb,
    body_digest bytea NOT NULL,
    files text NOT NULL,
    kept_at bigint NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    cause text DEFAULT 'auto'::text NOT NULL,
    body_bytes bigint DEFAULT 0 NOT NULL,
    actor_token_id uuid,
    actor_token_name text,
    CONSTRAINT body_versions_body_bytes_check CHECK ((body_bytes >= 0)),
    CONSTRAINT body_versions_cause_check CHECK ((cause = ANY (ARRAY['auto'::text, 'manual'::text, 'restored'::text, 'agent'::text])))
);

CREATE TABLE card_assignees (
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    container_id uuid NOT NULL,
    card_id uuid NOT NULL,
    user_id uuid NOT NULL
);

CREATE TABLE card_due_notices (
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    card_id uuid NOT NULL,
    user_id uuid NOT NULL,
    due_key text NOT NULL,
    notified_at bigint NOT NULL
);

CREATE TABLE categories (
    id uuid NOT NULL,
    name text NOT NULL,
    user_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    collapsed boolean DEFAULT false NOT NULL,
    type text NOT NULL,
    sort_order bigint DEFAULT 0 NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    CONSTRAINT categories_type_check CHECK ((type = ANY (ARRAY['system'::text, 'custom'::text])))
);

CREATE TABLE category_containers (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    category_id uuid NOT NULL,
    container_id uuid NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    sort_order bigint DEFAULT 0 NOT NULL,
    hidden boolean DEFAULT false NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL
);

CREATE TABLE comment_versions (
    block_id uuid NOT NULL,
    version bigint NOT NULL,
    body jsonb,
    files text NOT NULL,
    written_by uuid,
    written_at bigint NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL
);

CREATE TABLE container_members (
    container_id uuid NOT NULL,
    user_id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    role text NOT NULL,
    CONSTRAINT container_members_role_check CHECK ((role = ANY (ARRAY['admin'::text, 'editor'::text, 'commenter'::text, 'viewer'::text])))
);

CREATE TABLE container_teams (
    container_id uuid NOT NULL,
    team_id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    workspace_id uuid NOT NULL,
    role text NOT NULL,
    CONSTRAINT container_teams_role_check CHECK ((role = ANY (ARRAY['viewer'::text, 'commenter'::text, 'editor'::text])))
);

CREATE TABLE containers (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    created_by uuid,
    modified_by uuid,
    visibility text DEFAULT 'private'::text NOT NULL,
    minimum_role text DEFAULT 'editor'::text NOT NULL,
    title text NOT NULL,
    description text NOT NULL,
    icon text NOT NULL,
    show_description boolean NOT NULL,
    is_template boolean NOT NULL,
    is_builtin boolean DEFAULT false NOT NULL,
    template_version bigint DEFAULT 0 NOT NULL,
    properties jsonb NOT NULL,
    card_properties jsonb NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    kind text DEFAULT 'board'::text NOT NULL,
    next_card_number bigint DEFAULT 1 NOT NULL,
    due_date_property_id text,
    done_when jsonb,
    CONSTRAINT containers_kind_check CHECK ((kind = ANY (ARRAY['board'::text, 'wiki'::text, 'diary'::text, 'course'::text]))),
    CONSTRAINT containers_minimum_role_check CHECK ((minimum_role = ANY (ARRAY['viewer'::text, 'commenter'::text, 'editor'::text]))),
    CONSTRAINT containers_visibility_check CHECK ((visibility = ANY (ARRAY['open'::text, 'private'::text])))
);

CREATE TABLE course_exams (
    id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    container_id uuid NOT NULL,
    title text NOT NULL,
    exam_date text,
    max_score double precision NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    results_published_at bigint,
    published_average_score double precision,
    published_score_bucket bigint,
    published_submission_count bigint,
    CONSTRAINT course_exams_max_score_check CHECK (((max_score > (0)::double precision) AND (max_score <= (1000000000)::double precision)))
);

CREATE TABLE course_meetings (
    id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    container_id uuid NOT NULL,
    title text NOT NULL,
    starts_at bigint NOT NULL,
    location text DEFAULT ''::text NOT NULL,
    unit_ids text DEFAULT '[]'::text NOT NULL,
    presenter_id uuid,
    host_id uuid,
    notes text DEFAULT ''::text NOT NULL,
    notified_at bigint,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL
);

CREATE TABLE course_schedules (
    container_id uuid NOT NULL,
    start_date text NOT NULL,
    target_date text NOT NULL,
    study_days smallint NOT NULL,
    updated_at bigint NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    CONSTRAINT course_schedules_study_days_check CHECK (((study_days >= 1) AND (study_days <= 127)))
);

CREATE TABLE data_steps (
    name text NOT NULL
);

CREATE TABLE deleted_blocks (
    id uuid NOT NULL,
    container_id uuid NOT NULL,
    parent_id uuid,
    deleted_at bigint NOT NULL,
    deleted_by uuid,
    deletion_id uuid NOT NULL,
    block json NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    block_type text DEFAULT ''::text NOT NULL
);

CREATE TABLE deleted_containers (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    deleted_at bigint NOT NULL,
    deleted_by uuid,
    deletion_id uuid NOT NULL,
    container json NOT NULL,
    members json NOT NULL,
    files json NOT NULL,
    places json DEFAULT '[]'::json NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    team_roles jsonb DEFAULT '[]'::jsonb NOT NULL,
    next_card_number bigint DEFAULT 1 NOT NULL
);

CREATE TABLE diary_days (
    container_id uuid NOT NULL,
    day text NOT NULL,
    month_day text NOT NULL,
    entry_id uuid NOT NULL,
    mood text,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL
);

CREATE TABLE email_changes (
    user_id uuid NOT NULL,
    new_email text NOT NULL,
    token_hash text,
    token_expires_at bigint,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    CONSTRAINT email_changes_check CHECK (((token_hash IS NULL) = (token_expires_at IS NULL)))
);

CREATE TABLE email_reverts (
    token_hash text NOT NULL,
    user_id uuid NOT NULL,
    email text NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL
);

CREATE TABLE exam_scores (
    exam_id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    container_id uuid NOT NULL,
    user_id uuid NOT NULL,
    score double precision NOT NULL,
    note text DEFAULT ''::text NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT exam_scores_score_check CHECK (((score >= (0)::double precision) AND (score <= (1000000000)::double precision)))
);

CREATE TABLE file_refs (
    block_id uuid NOT NULL,
    container_id uuid NOT NULL,
    name text NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL
);

CREATE TABLE file_versions (
    file_id uuid NOT NULL,
    version bigint NOT NULL,
    path text NOT NULL,
    size bigint NOT NULL,
    created_at bigint NOT NULL,
    created_by uuid,
    restored_from bigint,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL
);

CREATE TABLE files (
    id uuid NOT NULL,
    container_id uuid NOT NULL,
    name text NOT NULL,
    original_name text NOT NULL,
    extension text NOT NULL,
    size bigint NOT NULL,
    path text NOT NULL,
    created_at bigint NOT NULL,
    unreferenced_at bigint,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    version bigint DEFAULT 1 NOT NULL,
    created_by uuid
);

CREATE TABLE instance (
    id smallint NOT NULL,
    bootstrapped boolean DEFAULT false NOT NULL,
    CONSTRAINT instance_id_check CHECK ((id = 1))
);

CREATE TABLE invitations (
    id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    workspace_id uuid NOT NULL,
    role text NOT NULL,
    container_id uuid,
    container_role text,
    token_hash text NOT NULL,
    created_by uuid NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    max_uses bigint,
    uses bigint DEFAULT 0 NOT NULL,
    revoked_at bigint DEFAULT 0 NOT NULL,
    parent_id uuid,
    recipient text,
    CONSTRAINT invitations_check CHECK (((role = 'guest'::text) = (container_id IS NOT NULL))),
    CONSTRAINT invitations_check1 CHECK (((role = 'guest'::text) = (container_role IS NOT NULL))),
    CONSTRAINT invitations_container_role_check CHECK ((container_role = ANY (ARRAY['viewer'::text, 'commenter'::text, 'editor'::text]))),
    CONSTRAINT invitations_max_uses_check CHECK ((max_uses > 0)),
    CONSTRAINT invitations_role_check CHECK ((role = ANY (ARRAY['member'::text, 'guest'::text])))
);

CREATE TABLE loose_objects (
    path text NOT NULL,
    state text NOT NULL,
    created_at bigint NOT NULL,
    retry_at bigint NOT NULL,
    attempts bigint DEFAULT 0 NOT NULL,
    last_attempt_at bigint DEFAULT 0 NOT NULL,
    last_error text DEFAULT ''::text NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    CONSTRAINT loose_objects_state_check CHECK ((state = ANY (ARRAY['writing'::text, 'doomed'::text])))
);

CREATE TABLE meeting_attendance (
    meeting_id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    user_id uuid NOT NULL,
    status text NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT meeting_attendance_status_check CHECK ((status = ANY (ARRAY['present'::text, 'late'::text, 'absent'::text, 'excused'::text])))
);

CREATE TABLE notification_outbox (
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    recipient_id uuid NOT NULL,
    kind text NOT NULL,
    source_id uuid NOT NULL,
    block_id uuid NOT NULL,
    container_id uuid NOT NULL,
    actor_id uuid,
    seq bigint NOT NULL,
    due_at bigint NOT NULL,
    attempts bigint DEFAULT 0 NOT NULL,
    actor_token_name text,
    CONSTRAINT notification_outbox_kind_check CHECK ((kind = ANY (ARRAY['mention'::text, 'assignment'::text, 'comment'::text, 'review_due'::text, 'meeting_upcoming'::text, 'due_soon'::text, 'added_to_container'::text])))
);

CREATE TABLE notifications (
    id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    recipient_id uuid NOT NULL,
    kind text NOT NULL,
    source_id uuid NOT NULL,
    block_id uuid NOT NULL,
    container_id uuid NOT NULL,
    actor_id uuid,
    created_at bigint NOT NULL,
    read_at bigint DEFAULT 0 NOT NULL,
    made_by uuid,
    updated_at bigint DEFAULT 0 NOT NULL,
    times bigint DEFAULT 1 NOT NULL,
    doers text DEFAULT '[]'::text NOT NULL,
    counted text DEFAULT '[]'::text NOT NULL,
    actor_token_name text,
    CONSTRAINT notifications_kind_check CHECK ((kind = ANY (ARRAY['mention'::text, 'assignment'::text, 'comment'::text, 'review_due'::text, 'meeting_upcoming'::text, 'due_soon'::text, 'added_to_container'::text])))
);

CREATE TABLE oidc_former_providers (
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    issuer text NOT NULL,
    client_id text NOT NULL,
    client_secret text NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL
);

CREATE TABLE oidc_held_sign_ins (
    held_hash text NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    provider_id uuid NOT NULL,
    revision bigint NOT NULL,
    issuer text NOT NULL,
    subject text NOT NULL,
    email text NOT NULL,
    email_unverified boolean NOT NULL,
    name text NOT NULL,
    preferred_username text NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL
);

CREATE TABLE oidc_login_flows (
    state_hash text NOT NULL,
    browser_hash text NOT NULL,
    nonce text NOT NULL,
    pkce_verifier text NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    provider_id uuid NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    link_session_id uuid,
    pending_at bigint,
    pending_by uuid,
    held_hash text
);

CREATE TABLE oidc_pending_providers (
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    provider_id uuid NOT NULL,
    issuer text NOT NULL,
    client_id text NOT NULL,
    client_secret text NOT NULL,
    created_at bigint NOT NULL
);

CREATE TABLE oidc_providers (
    id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    issuer text NOT NULL,
    client_id text NOT NULL,
    client_secret text NOT NULL,
    auto_join boolean DEFAULT true NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    sso_required boolean DEFAULT false NOT NULL,
    revision bigint DEFAULT 1 NOT NULL
);

CREATE TABLE organization_members (
    org_id uuid NOT NULL,
    user_id uuid NOT NULL,
    role text NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT organization_members_role_check CHECK ((role = ANY (ARRAY['admin'::text, 'member'::text])))
);

CREATE TABLE organizations (
    id uuid NOT NULL,
    name text NOT NULL,
    is_system boolean DEFAULT false NOT NULL,
    default_workspace_id uuid,
    password_login boolean DEFAULT true NOT NULL,
    settings json NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    deleted_at bigint,
    public_links boolean DEFAULT true NOT NULL,
    kind text DEFAULT 'shared'::text NOT NULL,
    owner_id uuid,
    created_by uuid,
    allow_copy_out boolean DEFAULT true NOT NULL,
    allow_api_tokens boolean DEFAULT false NOT NULL,
    CONSTRAINT organizations_deleted_at_check CHECK (((deleted_at IS NULL) OR (deleted_at > 0))),
    CONSTRAINT organizations_kind_check CHECK ((kind = ANY (ARRAY['shared'::text, 'personal'::text]))),
    CONSTRAINT organizations_owner_of_personal CHECK (((kind = 'personal'::text) = (owner_id IS NOT NULL))),
    CONSTRAINT organizations_system_is_no_ones CHECK (((NOT is_system) OR (kind = 'shared'::text)))
);

CREATE TABLE password_resets (
    token_hash text NOT NULL,
    user_id uuid NOT NULL,
    expires_at bigint NOT NULL
);

CREATE TABLE public_links (
    container_id uuid NOT NULL,
    enabled boolean NOT NULL,
    token text,
    modified_by uuid,
    updated_at bigint NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL
);

CREATE TABLE search_index_state (
    id smallint NOT NULL,
    name text NOT NULL,
    uuid text NOT NULL,
    version bigint NOT NULL,
    CONSTRAINT search_index_state_id_check CHECK ((id = 1))
);

CREATE TABLE search_outbox (
    scope text NOT NULL,
    id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    seq bigint NOT NULL,
    due_at bigint NOT NULL,
    attempts bigint DEFAULT 0 NOT NULL,
    CONSTRAINT search_outbox_scope_check CHECK ((scope = ANY (ARRAY['block'::text, 'container'::text])))
);

CREATE TABLE sessions (
    id uuid NOT NULL,
    token_hash text NOT NULL,
    user_id uuid NOT NULL,
    created_at bigint NOT NULL,
    last_used_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    user_agent text NOT NULL,
    login_provider_id uuid,
    login_org_id uuid,
    login_issuer text,
    login_revision bigint,
    login_subject text,
    password_confirmed_at bigint DEFAULT 0 NOT NULL,
    CONSTRAINT sessions_login_origin_check CHECK ((((login_provider_id IS NULL) AND (login_org_id IS NULL) AND (login_issuer IS NULL)) OR ((login_provider_id IS NOT NULL) AND (login_org_id IS NOT NULL) AND (login_issuer IS NOT NULL)))),
    CONSTRAINT sessions_login_revision_check CHECK (((login_revision IS NULL) OR (login_issuer IS NOT NULL))),
    CONSTRAINT sessions_login_subject_check CHECK (((login_subject IS NULL) OR (login_issuer IS NOT NULL)))
);

CREATE TABLE signups (
    id uuid NOT NULL,
    username text NOT NULL,
    email text NOT NULL,
    display_name text DEFAULT ''::text NOT NULL,
    password_hash text NOT NULL,
    language text DEFAULT ''::text NOT NULL,
    organization_name text,
    invitation_id uuid,
    invitation_org_id uuid,
    token_hash text,
    token_expires_at bigint,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    browser_hash text,
    CONSTRAINT signups_check CHECK (((organization_name IS NULL) <> (invitation_id IS NULL))),
    CONSTRAINT signups_check1 CHECK (((invitation_id IS NULL) = (invitation_org_id IS NULL))),
    CONSTRAINT signups_check2 CHECK (((token_hash IS NULL) = (token_expires_at IS NULL)))
);

CREATE TABLE subscriptions (
    block_type text NOT NULL,
    block_id uuid NOT NULL,
    user_id uuid NOT NULL,
    notified_at bigint DEFAULT 0 NOT NULL,
    created_at bigint NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL
);

CREATE TABLE team_members (
    team_id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    workspace_id uuid NOT NULL,
    user_id uuid NOT NULL
);

CREATE TABLE teams (
    id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    workspace_id uuid NOT NULL,
    name text NOT NULL,
    name_key text NOT NULL,
    CONSTRAINT teams_name_check CHECK ((length(name) > 0)),
    CONSTRAINT teams_name_key_check CHECK ((length(name_key) > 0))
);

CREATE TABLE time_entries (
    id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    user_id uuid NOT NULL,
    workspace_id uuid,
    container_id uuid,
    block_id uuid,
    target_kind text,
    target_title text,
    kind text NOT NULL,
    started_at bigint NOT NULL,
    ended_at bigint,
    note text DEFAULT ''::text NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT time_entries_check CHECK (((ended_at IS NULL) OR (ended_at >= started_at))),
    CONSTRAINT time_entries_kind_check CHECK ((kind = ANY (ARRAY['lecture'::text, 'study'::text, 'work'::text, 'other'::text]))),
    CONSTRAINT time_entries_target_kind_check CHECK (((target_kind IS NULL) OR (target_kind = ANY (ARRAY['card'::text, 'unit'::text, 'board'::text, 'course'::text]))))
);

CREATE TABLE unit_notes (
    id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    unit_id uuid NOT NULL,
    container_id uuid NOT NULL,
    user_id uuid NOT NULL,
    kind text NOT NULL,
    content text NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT unit_notes_kind_check CHECK ((kind = ANY (ARRAY['question'::text, 'mistake'::text, 'review'::text])))
);

CREATE TABLE unit_progress (
    unit_id uuid NOT NULL,
    container_id uuid NOT NULL,
    user_id uuid NOT NULL,
    completed_at bigint,
    planned_date text,
    last_position text,
    updated_at bigint NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL
);

CREATE TABLE unit_reviews (
    id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    unit_id uuid NOT NULL,
    container_id uuid NOT NULL,
    user_id uuid NOT NULL,
    scheduled_date text NOT NULL,
    completed_at bigint,
    interval_days integer NOT NULL,
    notified_at bigint,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT unit_reviews_interval_days_check CHECK (((interval_days >= 1) AND (interval_days <= 3650)))
);

CREATE TABLE upload_parts (
    upload_id text NOT NULL,
    number bigint NOT NULL,
    size bigint NOT NULL,
    part_id text NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL
);

CREATE TABLE uploads (
    id text NOT NULL,
    user_id uuid NOT NULL,
    container_id uuid NOT NULL,
    name text NOT NULL,
    size bigint NOT NULL,
    content_type text NOT NULL,
    part_size bigint NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL,
    closed_at bigint NOT NULL,
    multipart_id text NOT NULL,
    path text DEFAULT ''::text NOT NULL,
    purpose text DEFAULT 'file'::text NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL
);

CREATE TABLE user_client_state (
    user_id uuid NOT NULL,
    key text NOT NULL,
    value text NOT NULL
);

CREATE TABLE user_identities (
    issuer text NOT NULL,
    subject text NOT NULL,
    user_id uuid NOT NULL,
    email text NOT NULL,
    created_at bigint NOT NULL
);

CREATE TABLE user_settings (
    user_id uuid NOT NULL,
    language text DEFAULT 'auto'::text NOT NULL,
    theme text DEFAULT 'auto'::text NOT NULL,
    density text DEFAULT 'auto'::text NOT NULL,
    card_open text DEFAULT 'center'::text NOT NULL,
    haptics boolean DEFAULT true NOT NULL,
    week_start text DEFAULT 'auto'::text NOT NULL,
    hour_cycle text DEFAULT 'auto'::text NOT NULL,
    time_zone text DEFAULT 'auto'::text NOT NULL,
    notify_mentions boolean DEFAULT true NOT NULL,
    notify_assignments boolean DEFAULT true NOT NULL,
    notify_comments boolean DEFAULT true NOT NULL,
    notify_reviews boolean DEFAULT true NOT NULL,
    notify_meetings boolean DEFAULT true NOT NULL,
    notify_due boolean DEFAULT true NOT NULL,
    CONSTRAINT user_settings_card_open_check CHECK ((card_open = ANY (ARRAY['center'::text, 'side'::text, 'full'::text]))),
    CONSTRAINT user_settings_density_check CHECK ((density = ANY (ARRAY['auto'::text, 'compact'::text, 'comfortable'::text]))),
    CONSTRAINT user_settings_hour_cycle_check CHECK ((hour_cycle = ANY (ARRAY['auto'::text, 'h12'::text, 'h23'::text]))),
    CONSTRAINT user_settings_theme_check CHECK ((theme = ANY (ARRAY['auto'::text, 'light'::text, 'dark'::text]))),
    CONSTRAINT user_settings_week_start_check CHECK ((week_start = ANY (ARRAY['auto'::text, 'sunday'::text, 'monday'::text, 'saturday'::text])))
);

CREATE TABLE users (
    id uuid NOT NULL,
    username text NOT NULL,
    email text NOT NULL,
    password_hash text NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    deleted_at bigint,
    is_admin boolean DEFAULT false NOT NULL,
    display_name text DEFAULT ''::text NOT NULL,
    delete_after bigint DEFAULT 0 NOT NULL,
    erase_started_at bigint DEFAULT 0 NOT NULL,
    erased_at bigint DEFAULT 0 NOT NULL,
    first_name text DEFAULT ''::text NOT NULL,
    last_name text DEFAULT ''::text NOT NULL,
    latin_first_name text DEFAULT ''::text NOT NULL,
    latin_last_name text DEFAULT ''::text NOT NULL,
    custom_name text DEFAULT ''::text NOT NULL,
    name_shown text DEFAULT 'custom_name'::text NOT NULL,
    title text DEFAULT ''::text NOT NULL,
    credentials text DEFAULT ''::text NOT NULL,
    welcomed_at bigint,
    password_locked_at bigint DEFAULT 0 NOT NULL,
    email_verified_at bigint DEFAULT 0 NOT NULL,
    CONSTRAINT users_check2 CHECK (((erased_at = 0) OR (erase_started_at > 0))),
    CONSTRAINT users_delete_after_check CHECK (((delete_after = 0) OR ((deleted_at IS NOT NULL) AND (deleted_at > 0)))),
    CONSTRAINT users_deleted_at_check CHECK (((deleted_at IS NULL) OR (deleted_at > 0))),
    CONSTRAINT users_erase_started_at_check CHECK (((erase_started_at = 0) OR ((deleted_at IS NOT NULL) AND (deleted_at > 0))))
);

CREATE TABLE workspace_members (
    workspace_id uuid NOT NULL,
    user_id uuid NOT NULL,
    role text NOT NULL,
    via_org_admin boolean DEFAULT false NOT NULL,
    prior_role text,
    created_at bigint NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    prior_container_roles text,
    CONSTRAINT workspace_members_prior_role_check CHECK ((prior_role = ANY (ARRAY['admin'::text, 'member'::text, 'guest'::text]))),
    CONSTRAINT workspace_members_role_check CHECK ((role = ANY (ARRAY['admin'::text, 'member'::text, 'guest'::text])))
);

CREATE TABLE workspaces (
    id uuid NOT NULL,
    settings json NOT NULL,
    modified_by uuid,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    org_id uuid NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    is_system boolean DEFAULT false NOT NULL,
    deleted_at bigint,
    guest_invitations boolean DEFAULT true NOT NULL,
    welcomed_at bigint,
    CONSTRAINT workspaces_deleted_at_check CHECK (((deleted_at IS NULL) OR (deleted_at > 0))),
    CONSTRAINT workspaces_welcomed_at_check CHECK (((welcomed_at IS NULL) OR (welcomed_at > 0)))
);

INSERT INTO instance (id, bootstrapped) VALUES (1, FALSE);
INSERT INTO organizations (id, name, is_system, default_workspace_id, settings, created_at,
                           updated_at)
    VALUES ('00000000-0000-7000-8000-00000000a000', '', TRUE,
            '00000000-0000-7000-8000-00000000a001', '{}', 0, 0);
INSERT INTO workspaces (id, settings, created_at, updated_at, org_id, name, is_system)
    VALUES ('00000000-0000-7000-8000-00000000a001', '{}', 0, 0,
            '00000000-0000-7000-8000-00000000a000', '', TRUE);

ALTER TABLE api_token_workspaces ADD CONSTRAINT api_token_workspaces_pkey PRIMARY KEY (token_id, workspace_id);
ALTER TABLE api_tokens ADD CONSTRAINT api_tokens_pkey PRIMARY KEY (id);
ALTER TABLE audit_log ADD CONSTRAINT audit_log_pkey PRIMARY KEY (id, org_id);
ALTER TABLE blocks ADD CONSTRAINT blocks_container_id_id_key UNIQUE (container_id, id);
ALTER TABLE blocks ADD CONSTRAINT blocks_id_org_id_key UNIQUE (id, org_id);
ALTER TABLE blocks ADD CONSTRAINT blocks_pkey PRIMARY KEY (id);
ALTER TABLE body_docs ADD CONSTRAINT body_docs_pkey PRIMARY KEY (block_id, org_id);
ALTER TABLE body_links ADD CONSTRAINT body_links_pkey PRIMARY KEY (source_id, org_id, target_id);
ALTER TABLE body_updates ADD CONSTRAINT body_updates_pkey PRIMARY KEY (block_id, org_id, seq);
ALTER TABLE body_version_editors ADD CONSTRAINT body_version_editors_pkey PRIMARY KEY (block_id, org_id, version, user_id);
ALTER TABLE body_versions ADD CONSTRAINT body_versions_pkey PRIMARY KEY (block_id, org_id, version);
ALTER TABLE card_assignees ADD CONSTRAINT card_assignees_pkey PRIMARY KEY (org_id, card_id, user_id);
ALTER TABLE card_due_notices ADD CONSTRAINT card_due_notices_pkey PRIMARY KEY (org_id, card_id, user_id, due_key);
ALTER TABLE categories ADD CONSTRAINT categories_id_org_id_key UNIQUE (id, org_id);
ALTER TABLE categories ADD CONSTRAINT categories_pkey PRIMARY KEY (id);
ALTER TABLE category_containers ADD CONSTRAINT category_containers_pkey PRIMARY KEY (id);
ALTER TABLE category_containers ADD CONSTRAINT category_containers_user_id_container_id_org_id_key UNIQUE (user_id, container_id, org_id);
ALTER TABLE comment_versions ADD CONSTRAINT comment_versions_pkey PRIMARY KEY (block_id, org_id, version);
ALTER TABLE container_members ADD CONSTRAINT container_members_pkey PRIMARY KEY (container_id, user_id);
ALTER TABLE container_teams ADD CONSTRAINT container_teams_pkey PRIMARY KEY (container_id, team_id, org_id);
ALTER TABLE containers ADD CONSTRAINT containers_id_org_id_key UNIQUE (id, org_id);
ALTER TABLE containers ADD CONSTRAINT containers_pkey PRIMARY KEY (id);
ALTER TABLE course_exams ADD CONSTRAINT course_exams_pkey PRIMARY KEY (org_id, id);
ALTER TABLE course_meetings ADD CONSTRAINT course_meetings_pkey PRIMARY KEY (org_id, id);
ALTER TABLE course_schedules ADD CONSTRAINT course_schedules_pkey PRIMARY KEY (container_id, org_id);
ALTER TABLE data_steps ADD CONSTRAINT data_steps_pkey PRIMARY KEY (name);
ALTER TABLE deleted_blocks ADD CONSTRAINT deleted_blocks_pkey PRIMARY KEY (id, org_id);
ALTER TABLE deleted_containers ADD CONSTRAINT deleted_containers_pkey PRIMARY KEY (id, org_id);
ALTER TABLE diary_days ADD CONSTRAINT diary_days_entry_id_org_id_key UNIQUE (entry_id, org_id);
ALTER TABLE diary_days ADD CONSTRAINT diary_days_pkey PRIMARY KEY (container_id, day, org_id);
ALTER TABLE email_changes ADD CONSTRAINT email_changes_pkey PRIMARY KEY (user_id);
ALTER TABLE email_reverts ADD CONSTRAINT email_reverts_pkey PRIMARY KEY (token_hash);
ALTER TABLE exam_scores ADD CONSTRAINT exam_scores_pkey PRIMARY KEY (exam_id, org_id, user_id);
ALTER TABLE file_refs ADD CONSTRAINT file_refs_pkey PRIMARY KEY (block_id, org_id, name);
ALTER TABLE file_versions ADD CONSTRAINT file_versions_pkey PRIMARY KEY (file_id, org_id, version);
ALTER TABLE files ADD CONSTRAINT files_container_id_name_key UNIQUE (container_id, name);
ALTER TABLE files ADD CONSTRAINT files_pkey PRIMARY KEY (id);
ALTER TABLE instance ADD CONSTRAINT instance_pkey PRIMARY KEY (id);
ALTER TABLE invitations ADD CONSTRAINT invitations_pkey PRIMARY KEY (id, org_id);
ALTER TABLE loose_objects ADD CONSTRAINT loose_objects_pkey PRIMARY KEY (org_id, path);
ALTER TABLE meeting_attendance ADD CONSTRAINT meeting_attendance_pkey PRIMARY KEY (meeting_id, org_id, user_id);
ALTER TABLE notification_outbox ADD CONSTRAINT notification_outbox_pkey PRIMARY KEY (org_id, recipient_id, kind, source_id);
ALTER TABLE notifications ADD CONSTRAINT notifications_pkey PRIMARY KEY (org_id, id);
ALTER TABLE oidc_former_providers ADD CONSTRAINT oidc_former_providers_pkey PRIMARY KEY (org_id);
ALTER TABLE oidc_held_sign_ins ADD CONSTRAINT oidc_held_sign_ins_pkey PRIMARY KEY (held_hash);
ALTER TABLE oidc_login_flows ADD CONSTRAINT oidc_login_flows_pkey PRIMARY KEY (state_hash);
ALTER TABLE oidc_pending_providers ADD CONSTRAINT oidc_pending_providers_pkey PRIMARY KEY (org_id);
ALTER TABLE oidc_providers ADD CONSTRAINT oidc_providers_id_org_id_key UNIQUE (id, org_id);
ALTER TABLE oidc_providers ADD CONSTRAINT oidc_providers_pkey PRIMARY KEY (id);
ALTER TABLE organization_members ADD CONSTRAINT organization_members_pkey PRIMARY KEY (org_id, user_id);
ALTER TABLE organizations ADD CONSTRAINT organizations_pkey PRIMARY KEY (id);
ALTER TABLE password_resets ADD CONSTRAINT password_resets_pkey PRIMARY KEY (token_hash);
ALTER TABLE public_links ADD CONSTRAINT public_links_pkey PRIMARY KEY (container_id);
ALTER TABLE public_links ADD CONSTRAINT public_links_token_key UNIQUE (token);
ALTER TABLE search_index_state ADD CONSTRAINT search_index_state_pkey PRIMARY KEY (id);
ALTER TABLE search_outbox ADD CONSTRAINT search_outbox_pkey PRIMARY KEY (org_id, scope, id);
ALTER TABLE sessions ADD CONSTRAINT sessions_pkey PRIMARY KEY (id);
ALTER TABLE signups ADD CONSTRAINT signups_pkey PRIMARY KEY (id);
ALTER TABLE subscriptions ADD CONSTRAINT subscriptions_pkey PRIMARY KEY (block_id, user_id);
ALTER TABLE team_members ADD CONSTRAINT team_members_pkey PRIMARY KEY (team_id, user_id, org_id);
ALTER TABLE teams ADD CONSTRAINT teams_id_workspace_id_org_id_key UNIQUE (id, workspace_id, org_id);
ALTER TABLE teams ADD CONSTRAINT teams_pkey PRIMARY KEY (id, org_id);
ALTER TABLE teams ADD CONSTRAINT teams_workspace_id_name_key_org_id_key UNIQUE (workspace_id, name_key, org_id);
ALTER TABLE time_entries ADD CONSTRAINT time_entries_pkey PRIMARY KEY (id);
ALTER TABLE unit_notes ADD CONSTRAINT unit_notes_pkey PRIMARY KEY (org_id, id);
ALTER TABLE unit_progress ADD CONSTRAINT unit_progress_pkey PRIMARY KEY (unit_id, org_id, user_id);
ALTER TABLE unit_reviews ADD CONSTRAINT unit_reviews_pkey PRIMARY KEY (org_id, id);
ALTER TABLE upload_parts ADD CONSTRAINT upload_parts_pkey PRIMARY KEY (upload_id, number);
ALTER TABLE uploads ADD CONSTRAINT uploads_id_org_id_key UNIQUE (id, org_id);
ALTER TABLE uploads ADD CONSTRAINT uploads_pkey PRIMARY KEY (id);
ALTER TABLE user_client_state ADD CONSTRAINT user_client_state_pkey PRIMARY KEY (user_id, key);
ALTER TABLE user_identities ADD CONSTRAINT user_identities_pkey PRIMARY KEY (issuer, subject);
ALTER TABLE user_settings ADD CONSTRAINT user_settings_pkey PRIMARY KEY (user_id);
ALTER TABLE users ADD CONSTRAINT users_pkey PRIMARY KEY (id);
ALTER TABLE workspace_members ADD CONSTRAINT workspace_members_pkey PRIMARY KEY (workspace_id, user_id);
ALTER TABLE workspaces ADD CONSTRAINT workspaces_id_org_id_key UNIQUE (id, org_id);
ALTER TABLE workspaces ADD CONSTRAINT workspaces_pkey PRIMARY KEY (id);

CREATE INDEX api_token_workspaces_workspace_idx ON api_token_workspaces USING btree (org_id, workspace_id);
CREATE INDEX api_tokens_origin_provider_idx ON api_tokens USING btree (origin_provider_id) WHERE (origin_provider_id IS NOT NULL);
CREATE UNIQUE INDEX api_tokens_token_hash_key ON api_tokens USING btree (token_hash);
CREATE INDEX api_tokens_user_id_idx ON api_tokens USING btree (user_id);
CREATE INDEX audit_log_actor_identity_idx ON audit_log USING btree (actor_id) WHERE (identity IS NOT NULL);
CREATE INDEX audit_log_org_idx ON audit_log USING btree (org_id, created_at) WHERE (workspace_id IS NULL);
CREATE INDEX audit_log_subject_identity_idx ON audit_log USING btree (subject_id) WHERE (identity IS NOT NULL);
CREATE INDEX audit_log_workspace_id_idx ON audit_log USING btree (workspace_id, created_at);
CREATE UNIQUE INDEX blocks_container_id_number_idx ON blocks USING btree (container_id, number);
CREATE INDEX blocks_container_id_parent_id_idx ON blocks USING btree (container_id, parent_id);
CREATE INDEX blocks_container_titled_updated_at_idx ON blocks USING btree (container_id, updated_at DESC, id) WHERE (type = ANY (ARRAY['card'::text, 'page'::text, 'unit'::text]));
CREATE INDEX blocks_container_type_created_at_idx ON blocks USING btree (container_id, type, created_at, id);
CREATE INDEX blocks_created_by_idx ON blocks USING btree (created_by);
CREATE INDEX blocks_due_at_idx ON blocks USING btree (container_id, due_at) WHERE ((due_at IS NOT NULL) AND (is_done = false));
CREATE INDEX blocks_modified_by_idx ON blocks USING btree (modified_by);
CREATE INDEX blocks_parent_type_created_at_idx ON blocks USING btree (parent_id, type, created_at);
CREATE INDEX body_links_target_idx ON body_links USING btree (target_id, org_id);
CREATE INDEX body_version_editors_user_id_idx ON body_version_editors USING btree (user_id);
CREATE INDEX card_assignees_container_idx ON card_assignees USING btree (container_id);
CREATE INDEX card_assignees_user_container_idx ON card_assignees USING btree (org_id, user_id, container_id);
CREATE INDEX card_assignees_user_idx ON card_assignees USING btree (user_id);
CREATE INDEX card_due_notices_card_idx ON card_due_notices USING btree (card_id);
CREATE INDEX card_due_notices_user_idx ON card_due_notices USING btree (user_id);
CREATE INDEX categories_user_id_workspace_id_idx ON categories USING btree (user_id, workspace_id);
CREATE INDEX category_containers_category_id_idx ON category_containers USING btree (category_id);
CREATE INDEX category_containers_container_id_idx ON category_containers USING btree (container_id);
CREATE INDEX comment_versions_written_by_idx ON comment_versions USING btree (written_by);
CREATE INDEX container_members_user_id_idx ON container_members USING btree (user_id);
CREATE INDEX container_teams_team_idx ON container_teams USING btree (team_id, org_id);
CREATE INDEX containers_created_by_idx ON containers USING btree (created_by);
CREATE INDEX containers_workspace_id_is_template_idx ON containers USING btree (workspace_id, is_template);
CREATE UNIQUE INDEX containers_workspace_scope_key ON containers USING btree (id, workspace_id, org_id);
CREATE INDEX course_exams_container_idx ON course_exams USING btree (org_id, container_id, exam_date, created_at, id);
CREATE INDEX course_meetings_container_idx ON course_meetings USING btree (org_id, container_id, starts_at, id);
CREATE INDEX course_meetings_upcoming_idx ON course_meetings USING btree (starts_at) WHERE (notified_at IS NULL);
CREATE INDEX deleted_blocks_container_deleted_at_idx ON deleted_blocks USING btree (container_id, deleted_at DESC, id);
CREATE INDEX deleted_blocks_container_type_deleted_at_idx ON deleted_blocks USING btree (container_id, block_type, deleted_at DESC, id);
CREATE INDEX deleted_blocks_deleted_at_idx ON deleted_blocks USING btree (deleted_at);
CREATE INDEX deleted_blocks_deletion_id_idx ON deleted_blocks USING btree (deletion_id);
CREATE INDEX deleted_containers_deleted_at_idx ON deleted_containers USING btree (deleted_at);
CREATE INDEX deleted_containers_workspace_id_idx ON deleted_containers USING btree (workspace_id, deleted_at);
CREATE INDEX diary_days_month_day_idx ON diary_days USING btree (container_id, month_day);
CREATE INDEX email_changes_expires_at_idx ON email_changes USING btree (expires_at);
CREATE UNIQUE INDEX email_changes_token_hash_key ON email_changes USING btree (token_hash);
CREATE INDEX email_reverts_email_idx ON email_reverts USING btree (lower(email));
CREATE INDEX email_reverts_expires_at_idx ON email_reverts USING btree (expires_at);
CREATE INDEX email_reverts_user_id_idx ON email_reverts USING btree (user_id, created_at);
CREATE INDEX exam_scores_container_user_idx ON exam_scores USING btree (org_id, container_id, user_id);
CREATE INDEX exam_scores_user_idx ON exam_scores USING btree (user_id);
CREATE INDEX file_refs_container_id_name_idx ON file_refs USING btree (container_id, name);
CREATE INDEX file_versions_created_by_idx ON file_versions USING btree (created_by);
CREATE INDEX file_versions_path_idx ON file_versions USING btree (path);
CREATE INDEX files_created_by_idx ON files USING btree (created_by);
CREATE INDEX files_path_idx ON files USING btree (path);
CREATE INDEX files_unreferenced_at_idx ON files USING btree (unreferenced_at);
CREATE INDEX invitations_created_by_idx ON invitations USING btree (created_by);
CREATE INDEX invitations_parent_id_idx ON invitations USING btree (parent_id) WHERE (parent_id IS NOT NULL);
CREATE UNIQUE INDEX invitations_token_hash_key ON invitations USING btree (token_hash);
CREATE INDEX invitations_workspace_id_idx ON invitations USING btree (workspace_id, created_at);
CREATE INDEX loose_objects_path_idx ON loose_objects USING btree (path);
CREATE INDEX loose_objects_state_retry_at_idx ON loose_objects USING btree (state, retry_at);
CREATE INDEX meeting_attendance_meeting_idx ON meeting_attendance USING btree (org_id, meeting_id);
CREATE INDEX meeting_attendance_user_idx ON meeting_attendance USING btree (user_id);
CREATE INDEX notification_outbox_block_idx ON notification_outbox USING btree (block_id);
CREATE INDEX notification_outbox_container_idx ON notification_outbox USING btree (container_id);
CREATE INDEX notification_outbox_due_at_idx ON notification_outbox USING btree (due_at);
CREATE INDEX notifications_block_idx ON notifications USING btree (block_id);
CREATE INDEX notifications_container_idx ON notifications USING btree (container_id);
CREATE INDEX notifications_id_idx ON notifications USING btree (id);
CREATE INDEX notifications_made_by_idx ON notifications USING btree (recipient_id, made_by, created_at);
CREATE INDEX notifications_read_at_idx ON notifications USING btree (read_at) WHERE (read_at > 0);
CREATE INDEX notifications_recipient_idx ON notifications USING btree (recipient_id, updated_at DESC, id DESC);
CREATE INDEX notifications_same_idx ON notifications USING btree (recipient_id, block_id, kind, created_at);
CREATE INDEX notifications_unread_idx ON notifications USING btree (recipient_id) WHERE (read_at = 0);
CREATE INDEX oidc_former_providers_expires_at_idx ON oidc_former_providers USING btree (expires_at);
CREATE INDEX oidc_held_sign_ins_expires_at_idx ON oidc_held_sign_ins USING btree (expires_at);
CREATE INDEX oidc_login_flows_expires_at_idx ON oidc_login_flows USING btree (expires_at);
CREATE UNIQUE INDEX oidc_providers_org_id_key ON oidc_providers USING btree (org_id);
CREATE INDEX organization_members_user_id_idx ON organization_members USING btree (user_id);
CREATE INDEX organizations_created_by_idx ON organizations USING btree (created_by) WHERE (created_by IS NOT NULL);
CREATE INDEX organizations_of_people_idx ON organizations USING btree (created_at, id) WHERE (kind = 'shared'::text);
CREATE UNIQUE INDEX organizations_owner_id_key ON organizations USING btree (owner_id) WHERE (owner_id IS NOT NULL);
CREATE UNIQUE INDEX organizations_system_key ON organizations USING btree (is_system) WHERE is_system;
CREATE INDEX password_resets_expires_at_idx ON password_resets USING btree (expires_at);
CREATE INDEX password_resets_user_id_idx ON password_resets USING btree (user_id);
CREATE INDEX search_outbox_due_at_idx ON search_outbox USING btree (due_at);
CREATE INDEX sessions_login_origin_idx ON sessions USING btree (login_org_id, login_issuer) WHERE (login_issuer IS NOT NULL);
CREATE UNIQUE INDEX sessions_token_hash_key ON sessions USING btree (token_hash);
CREATE INDEX sessions_user_id_idx ON sessions USING btree (user_id);
CREATE UNIQUE INDEX signups_browser_hash_key ON signups USING btree (browser_hash);
CREATE UNIQUE INDEX signups_email_key ON signups USING btree (lower(email));
CREATE INDEX signups_expires_at_idx ON signups USING btree (expires_at);
CREATE INDEX signups_invitation_idx ON signups USING btree (invitation_id, invitation_org_id) WHERE (invitation_id IS NOT NULL);
CREATE UNIQUE INDEX signups_token_hash_key ON signups USING btree (token_hash);
CREATE UNIQUE INDEX signups_username_key ON signups USING btree (lower(username));
CREATE INDEX subscriptions_user_id_idx ON subscriptions USING btree (user_id);
CREATE INDEX team_members_user_idx ON team_members USING btree (user_id, workspace_id, org_id);
CREATE UNIQUE INDEX time_entries_one_active_per_user ON time_entries USING btree (user_id) WHERE (ended_at IS NULL);
CREATE INDEX time_entries_org_user_started_idx ON time_entries USING btree (org_id, user_id, started_at DESC);
CREATE INDEX time_entries_user_block_started_idx ON time_entries USING btree (user_id, block_id, started_at DESC, id DESC) WHERE (block_id IS NOT NULL);
CREATE INDEX time_entries_user_container_started_idx ON time_entries USING btree (user_id, container_id, started_at DESC, id DESC) WHERE (container_id IS NOT NULL);
CREATE INDEX time_entries_user_started_idx ON time_entries USING btree (user_id, started_at DESC, id DESC);
CREATE INDEX unit_notes_container_user_unit_created_idx ON unit_notes USING btree (org_id, container_id, user_id, unit_id, created_at DESC);
CREATE INDEX unit_notes_user_idx ON unit_notes USING btree (user_id);
CREATE INDEX unit_progress_container_completed_idx ON unit_progress USING btree (org_id, container_id, completed_at);
CREATE INDEX unit_progress_container_user_idx ON unit_progress USING btree (org_id, container_id, user_id);
CREATE INDEX unit_progress_user_idx ON unit_progress USING btree (user_id);
CREATE INDEX unit_reviews_container_user_unit_idx ON unit_reviews USING btree (org_id, container_id, user_id, unit_id);
CREATE INDEX unit_reviews_due_notify_idx ON unit_reviews USING btree (scheduled_date, created_at, id) WHERE ((completed_at IS NULL) AND (notified_at IS NULL));
CREATE INDEX unit_reviews_user_idx ON unit_reviews USING btree (user_id);
CREATE INDEX unit_reviews_user_scheduled_idx ON unit_reviews USING btree (org_id, user_id, scheduled_date);
CREATE INDEX uploads_expires_at_idx ON uploads USING btree (expires_at);
CREATE INDEX uploads_path_idx ON uploads USING btree (path);
CREATE INDEX uploads_user_id_idx ON uploads USING btree (user_id);
CREATE INDEX user_identities_user_id_idx ON user_identities USING btree (user_id);
CREATE UNIQUE INDEX users_email_key ON users USING btree (lower(email)) WHERE (email <> ''::text);
CREATE INDEX users_leaving_idx ON users USING btree (delete_after) WHERE ((deleted_at IS NOT NULL) AND (erased_at = 0));
CREATE UNIQUE INDEX users_username_key ON users USING btree (lower(username)) WHERE (username <> ''::text);
CREATE UNIQUE INDEX workspace_members_scope_key ON workspace_members USING btree (workspace_id, user_id, org_id);
CREATE INDEX workspace_members_user_id_idx ON workspace_members USING btree (user_id);
CREATE INDEX workspaces_deleted_idx ON workspaces USING btree (deleted_at) WHERE (deleted_at IS NOT NULL);
CREATE UNIQUE INDEX workspaces_name_in_use_key ON workspaces USING btree (org_id, name) WHERE ((deleted_at IS NULL) AND (name <> ''::text));
CREATE INDEX workspaces_org_id_idx ON workspaces USING btree (org_id);
CREATE UNIQUE INDEX workspaces_scope_key ON workspaces USING btree (id, org_id);

ALTER TABLE api_token_workspaces ADD CONSTRAINT api_token_workspaces_org_id_fkey FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;
ALTER TABLE api_token_workspaces ADD CONSTRAINT api_token_workspaces_token_id_fkey FOREIGN KEY (token_id) REFERENCES api_tokens(id) ON DELETE CASCADE;
ALTER TABLE api_token_workspaces ADD CONSTRAINT api_token_workspaces_workspace_id_org_id_fkey FOREIGN KEY (workspace_id, org_id) REFERENCES workspaces(id, org_id) ON DELETE CASCADE;
ALTER TABLE api_tokens ADD CONSTRAINT api_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE audit_log ADD CONSTRAINT audit_log_org_fkey FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;
ALTER TABLE audit_log ADD CONSTRAINT audit_log_workspace_id_org_id_fkey FOREIGN KEY (workspace_id, org_id) REFERENCES workspaces(id, org_id) ON DELETE CASCADE;
ALTER TABLE blocks ADD CONSTRAINT blocks_container_id_fkey FOREIGN KEY (container_id) REFERENCES containers(id);
ALTER TABLE blocks ADD CONSTRAINT blocks_container_id_parent_id_fkey FOREIGN KEY (container_id, parent_id) REFERENCES blocks(container_id, id);
ALTER TABLE blocks ADD CONSTRAINT blocks_container_org_fkey FOREIGN KEY (container_id, org_id) REFERENCES containers(id, org_id);
ALTER TABLE blocks ADD CONSTRAINT blocks_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id);
ALTER TABLE blocks ADD CONSTRAINT blocks_modified_by_fkey FOREIGN KEY (modified_by) REFERENCES users(id);
ALTER TABLE body_updates ADD CONSTRAINT body_updates_block_id_org_id_fkey FOREIGN KEY (block_id, org_id) REFERENCES body_docs(block_id, org_id) ON DELETE CASCADE;
ALTER TABLE card_assignees ADD CONSTRAINT card_assignees_container_id_card_id_fkey FOREIGN KEY (container_id, card_id) REFERENCES blocks(container_id, id) ON DELETE CASCADE;
ALTER TABLE card_assignees ADD CONSTRAINT card_assignees_container_id_org_id_fkey FOREIGN KEY (container_id, org_id) REFERENCES containers(id, org_id) ON DELETE CASCADE;
ALTER TABLE card_assignees ADD CONSTRAINT card_assignees_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE card_due_notices ADD CONSTRAINT card_due_notices_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE categories ADD CONSTRAINT categories_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE categories ADD CONSTRAINT categories_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES workspaces(id);
ALTER TABLE categories ADD CONSTRAINT categories_workspace_org_fkey FOREIGN KEY (workspace_id, org_id) REFERENCES workspaces(id, org_id);
ALTER TABLE category_containers ADD CONSTRAINT category_containers_category_id_fkey FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE CASCADE;
ALTER TABLE category_containers ADD CONSTRAINT category_containers_category_org_fkey FOREIGN KEY (category_id, org_id) REFERENCES categories(id, org_id) ON DELETE CASCADE;
ALTER TABLE category_containers ADD CONSTRAINT category_containers_container_id_fkey FOREIGN KEY (container_id) REFERENCES containers(id) ON DELETE CASCADE;
ALTER TABLE category_containers ADD CONSTRAINT category_containers_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE container_members ADD CONSTRAINT container_members_container_id_fkey FOREIGN KEY (container_id) REFERENCES containers(id) ON DELETE CASCADE;
ALTER TABLE container_members ADD CONSTRAINT container_members_container_org_fkey FOREIGN KEY (container_id, org_id) REFERENCES containers(id, org_id) ON DELETE CASCADE;
ALTER TABLE container_members ADD CONSTRAINT container_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE container_teams ADD CONSTRAINT container_teams_container_id_workspace_id_org_id_fkey FOREIGN KEY (container_id, workspace_id, org_id) REFERENCES containers(id, workspace_id, org_id) ON DELETE CASCADE;
ALTER TABLE container_teams ADD CONSTRAINT container_teams_team_id_workspace_id_org_id_fkey FOREIGN KEY (team_id, workspace_id, org_id) REFERENCES teams(id, workspace_id, org_id) ON DELETE CASCADE;
ALTER TABLE containers ADD CONSTRAINT containers_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id);
ALTER TABLE containers ADD CONSTRAINT containers_modified_by_fkey FOREIGN KEY (modified_by) REFERENCES users(id);
ALTER TABLE containers ADD CONSTRAINT containers_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES workspaces(id);
ALTER TABLE containers ADD CONSTRAINT containers_workspace_org_fkey FOREIGN KEY (workspace_id, org_id) REFERENCES workspaces(id, org_id);
ALTER TABLE diary_days ADD CONSTRAINT diary_days_entry_id_org_id_fkey FOREIGN KEY (entry_id, org_id) REFERENCES blocks(id, org_id) ON DELETE CASCADE;
ALTER TABLE email_changes ADD CONSTRAINT email_changes_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE email_reverts ADD CONSTRAINT email_reverts_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE files ADD CONSTRAINT files_container_id_fkey FOREIGN KEY (container_id) REFERENCES containers(id) ON DELETE CASCADE;
ALTER TABLE files ADD CONSTRAINT files_container_org_fkey FOREIGN KEY (container_id, org_id) REFERENCES containers(id, org_id) ON DELETE CASCADE;
ALTER TABLE invitations ADD CONSTRAINT invitations_container_id_org_id_fkey FOREIGN KEY (container_id, org_id) REFERENCES containers(id, org_id) ON DELETE CASCADE;
ALTER TABLE invitations ADD CONSTRAINT invitations_parent_fkey FOREIGN KEY (parent_id, org_id) REFERENCES invitations(id, org_id) ON DELETE CASCADE;
ALTER TABLE invitations ADD CONSTRAINT invitations_workspace_id_org_id_fkey FOREIGN KEY (workspace_id, org_id) REFERENCES workspaces(id, org_id) ON DELETE CASCADE;
ALTER TABLE oidc_former_providers ADD CONSTRAINT oidc_former_providers_org_id_fkey FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;
ALTER TABLE oidc_held_sign_ins ADD CONSTRAINT oidc_held_sign_ins_org_id_fkey FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;
ALTER TABLE oidc_held_sign_ins ADD CONSTRAINT oidc_held_sign_ins_provider_id_org_id_fkey FOREIGN KEY (provider_id, org_id) REFERENCES oidc_providers(id, org_id) ON DELETE CASCADE;
ALTER TABLE oidc_login_flows ADD CONSTRAINT oidc_login_flows_org_id_fkey FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;
ALTER TABLE oidc_login_flows ADD CONSTRAINT oidc_login_flows_provider_id_org_id_fkey FOREIGN KEY (provider_id, org_id) REFERENCES oidc_providers(id, org_id) ON DELETE CASCADE;
ALTER TABLE oidc_pending_providers ADD CONSTRAINT oidc_pending_providers_org_id_fkey FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;
ALTER TABLE oidc_pending_providers ADD CONSTRAINT oidc_pending_providers_provider_id_org_id_fkey FOREIGN KEY (provider_id, org_id) REFERENCES oidc_providers(id, org_id) ON DELETE CASCADE;
ALTER TABLE oidc_providers ADD CONSTRAINT oidc_providers_org_id_fkey FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;
ALTER TABLE organization_members ADD CONSTRAINT organization_members_org_id_fkey FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;
ALTER TABLE organization_members ADD CONSTRAINT organization_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE organizations ADD CONSTRAINT organizations_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE organizations ADD CONSTRAINT organizations_default_workspace_id_fkey FOREIGN KEY (default_workspace_id) REFERENCES workspaces(id);
ALTER TABLE organizations ADD CONSTRAINT organizations_default_workspace_org_fkey FOREIGN KEY (default_workspace_id, id) REFERENCES workspaces(id, org_id);
ALTER TABLE organizations ADD CONSTRAINT organizations_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES users(id);
ALTER TABLE password_resets ADD CONSTRAINT password_resets_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE public_links ADD CONSTRAINT public_links_container_id_fkey FOREIGN KEY (container_id) REFERENCES containers(id) ON DELETE CASCADE;
ALTER TABLE public_links ADD CONSTRAINT public_links_container_org_fkey FOREIGN KEY (container_id, org_id) REFERENCES containers(id, org_id) ON DELETE CASCADE;
ALTER TABLE public_links ADD CONSTRAINT public_links_modified_by_fkey FOREIGN KEY (modified_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE sessions ADD CONSTRAINT sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE signups ADD CONSTRAINT signups_invitation_id_invitation_org_id_fkey FOREIGN KEY (invitation_id, invitation_org_id) REFERENCES invitations(id, org_id) ON DELETE CASCADE;
ALTER TABLE subscriptions ADD CONSTRAINT subscriptions_block_id_fkey FOREIGN KEY (block_id) REFERENCES blocks(id) ON DELETE CASCADE;
ALTER TABLE subscriptions ADD CONSTRAINT subscriptions_block_org_fkey FOREIGN KEY (block_id, org_id) REFERENCES blocks(id, org_id) ON DELETE CASCADE;
ALTER TABLE subscriptions ADD CONSTRAINT subscriptions_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE team_members ADD CONSTRAINT team_members_team_id_workspace_id_org_id_fkey FOREIGN KEY (team_id, workspace_id, org_id) REFERENCES teams(id, workspace_id, org_id) ON DELETE CASCADE;
ALTER TABLE team_members ADD CONSTRAINT team_members_workspace_id_user_id_org_id_fkey FOREIGN KEY (workspace_id, user_id, org_id) REFERENCES workspace_members(workspace_id, user_id, org_id) ON DELETE CASCADE;
ALTER TABLE teams ADD CONSTRAINT teams_org_id_fkey FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;
ALTER TABLE teams ADD CONSTRAINT teams_workspace_id_org_id_fkey FOREIGN KEY (workspace_id, org_id) REFERENCES workspaces(id, org_id) ON DELETE CASCADE;
ALTER TABLE upload_parts ADD CONSTRAINT upload_parts_upload_id_fkey FOREIGN KEY (upload_id) REFERENCES uploads(id) ON DELETE CASCADE;
ALTER TABLE upload_parts ADD CONSTRAINT upload_parts_upload_org_fkey FOREIGN KEY (upload_id, org_id) REFERENCES uploads(id, org_id) ON DELETE CASCADE;
ALTER TABLE uploads ADD CONSTRAINT uploads_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE user_client_state ADD CONSTRAINT user_client_state_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE user_identities ADD CONSTRAINT user_identities_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE user_settings ADD CONSTRAINT user_settings_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE workspace_members ADD CONSTRAINT workspace_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE workspace_members ADD CONSTRAINT workspace_members_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE;
ALTER TABLE workspace_members ADD CONSTRAINT workspace_members_workspace_org_fkey FOREIGN KEY (workspace_id, org_id) REFERENCES workspaces(id, org_id) ON DELETE CASCADE;
ALTER TABLE workspaces ADD CONSTRAINT workspaces_modified_by_fkey FOREIGN KEY (modified_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE workspaces ADD CONSTRAINT workspaces_org_id_fkey FOREIGN KEY (org_id) REFERENCES organizations(id);

CREATE TRIGGER audit_log_kept BEFORE DELETE OR UPDATE ON audit_log FOR EACH ROW EXECUTE FUNCTION audit_log_is_kept();

CREATE TRIGGER category_containers_container_of_its_organization BEFORE INSERT OR UPDATE OF container_id, org_id ON category_containers FOR EACH ROW EXECUTE FUNCTION category_containers_container_of_its_organization();

CREATE TRIGGER course_exams_course_of_its_organization BEFORE INSERT OR UPDATE OF container_id, org_id ON course_exams FOR EACH ROW EXECUTE FUNCTION course_child_of_its_course();

CREATE TRIGGER course_meetings_course_of_its_organization BEFORE INSERT OR UPDATE OF container_id, org_id ON course_meetings FOR EACH ROW EXECUTE FUNCTION course_child_of_its_course();

CREATE TRIGGER diaries_of_a_space BEFORE INSERT OR UPDATE OF org_id, workspace_id, kind ON containers FOR EACH ROW EXECUTE FUNCTION diaries_are_of_a_space();

CREATE TRIGGER exam_scores_course_of_its_organization BEFORE INSERT OR UPDATE OF container_id, org_id ON exam_scores FOR EACH ROW EXECUTE FUNCTION course_child_of_its_course();

CREATE TRIGGER invitations_of_a_space BEFORE INSERT OR UPDATE OF org_id ON invitations FOR EACH ROW EXECUTE FUNCTION spaces_have_no_invitations();

CREATE TRIGGER organization_members_of_a_space BEFORE INSERT OR UPDATE OF org_id, user_id ON organization_members FOR EACH ROW EXECUTE FUNCTION spaces_take_their_owner_alone();

CREATE TRIGGER organizations_keep_their_kind BEFORE UPDATE OF kind, owner_id ON organizations FOR EACH ROW WHEN (((new.kind IS DISTINCT FROM old.kind) OR (new.owner_id IS DISTINCT FROM old.owner_id))) EXECUTE FUNCTION organizations_keep_their_kind();

CREATE TRIGGER public_links_of_no_diary BEFORE INSERT OR UPDATE OF org_id, container_id ON public_links FOR EACH ROW EXECUTE FUNCTION diaries_have_no_link();

CREATE TRIGGER unit_notes_course_of_its_organization BEFORE INSERT OR UPDATE OF container_id, org_id ON unit_notes FOR EACH ROW EXECUTE FUNCTION course_child_of_its_course();

CREATE TRIGGER unit_progress_course_of_its_organization BEFORE INSERT OR UPDATE OF container_id, org_id ON unit_progress FOR EACH ROW EXECUTE FUNCTION course_child_of_its_course();

CREATE TRIGGER unit_reviews_course_of_its_organization BEFORE INSERT OR UPDATE OF container_id, org_id ON unit_reviews FOR EACH ROW EXECUTE FUNCTION course_child_of_its_course();

CREATE TRIGGER uploads_target_of_its_organization BEFORE INSERT OR UPDATE OF container_id, purpose, org_id ON uploads FOR EACH ROW EXECUTE FUNCTION uploads_target_of_its_organization();

CREATE TRIGGER workspace_members_of_a_space BEFORE INSERT OR UPDATE OF org_id, user_id ON workspace_members FOR EACH ROW EXECUTE FUNCTION spaces_take_their_owner_alone();

ALTER TABLE api_token_workspaces ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE blocks ENABLE ROW LEVEL SECURITY;
ALTER TABLE body_docs ENABLE ROW LEVEL SECURITY;
ALTER TABLE body_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE body_updates ENABLE ROW LEVEL SECURITY;
ALTER TABLE body_version_editors ENABLE ROW LEVEL SECURITY;
ALTER TABLE body_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE card_assignees ENABLE ROW LEVEL SECURITY;
ALTER TABLE card_due_notices ENABLE ROW LEVEL SECURITY;
ALTER TABLE categories ENABLE ROW LEVEL SECURITY;
ALTER TABLE category_containers ENABLE ROW LEVEL SECURITY;
ALTER TABLE comment_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE container_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE container_teams ENABLE ROW LEVEL SECURITY;
ALTER TABLE containers ENABLE ROW LEVEL SECURITY;
ALTER TABLE course_exams ENABLE ROW LEVEL SECURITY;
ALTER TABLE course_meetings ENABLE ROW LEVEL SECURITY;
ALTER TABLE course_schedules ENABLE ROW LEVEL SECURITY;
ALTER TABLE deleted_blocks ENABLE ROW LEVEL SECURITY;
ALTER TABLE deleted_containers ENABLE ROW LEVEL SECURITY;
ALTER TABLE diary_days ENABLE ROW LEVEL SECURITY;
ALTER TABLE exam_scores ENABLE ROW LEVEL SECURITY;
ALTER TABLE file_refs ENABLE ROW LEVEL SECURITY;
ALTER TABLE file_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE files ENABLE ROW LEVEL SECURITY;
ALTER TABLE invitations ENABLE ROW LEVEL SECURITY;
ALTER TABLE loose_objects ENABLE ROW LEVEL SECURITY;
ALTER TABLE meeting_attendance ENABLE ROW LEVEL SECURITY;
ALTER TABLE notification_outbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE notifications ENABLE ROW LEVEL SECURITY;
ALTER TABLE oidc_former_providers ENABLE ROW LEVEL SECURITY;
ALTER TABLE oidc_held_sign_ins ENABLE ROW LEVEL SECURITY;
ALTER TABLE oidc_login_flows ENABLE ROW LEVEL SECURITY;
ALTER TABLE oidc_pending_providers ENABLE ROW LEVEL SECURITY;
ALTER TABLE oidc_providers ENABLE ROW LEVEL SECURITY;
ALTER TABLE organization_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE organizations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE search_outbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE subscriptions ENABLE ROW LEVEL SECURITY;
ALTER TABLE team_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE teams ENABLE ROW LEVEL SECURITY;
ALTER TABLE time_entries ENABLE ROW LEVEL SECURITY;
ALTER TABLE unit_notes ENABLE ROW LEVEL SECURITY;
ALTER TABLE unit_progress ENABLE ROW LEVEL SECURITY;
ALTER TABLE unit_reviews ENABLE ROW LEVEL SECURITY;
ALTER TABLE upload_parts ENABLE ROW LEVEL SECURITY;
ALTER TABLE uploads ENABLE ROW LEVEL SECURITY;
ALTER TABLE workspace_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE workspaces ENABLE ROW LEVEL SECURITY;

ALTER TABLE api_token_workspaces FORCE ROW LEVEL SECURITY;
ALTER TABLE audit_log FORCE ROW LEVEL SECURITY;
ALTER TABLE blocks FORCE ROW LEVEL SECURITY;
ALTER TABLE body_docs FORCE ROW LEVEL SECURITY;
ALTER TABLE body_links FORCE ROW LEVEL SECURITY;
ALTER TABLE body_updates FORCE ROW LEVEL SECURITY;
ALTER TABLE body_version_editors FORCE ROW LEVEL SECURITY;
ALTER TABLE body_versions FORCE ROW LEVEL SECURITY;
ALTER TABLE card_assignees FORCE ROW LEVEL SECURITY;
ALTER TABLE card_due_notices FORCE ROW LEVEL SECURITY;
ALTER TABLE categories FORCE ROW LEVEL SECURITY;
ALTER TABLE category_containers FORCE ROW LEVEL SECURITY;
ALTER TABLE comment_versions FORCE ROW LEVEL SECURITY;
ALTER TABLE container_members FORCE ROW LEVEL SECURITY;
ALTER TABLE container_teams FORCE ROW LEVEL SECURITY;
ALTER TABLE containers FORCE ROW LEVEL SECURITY;
ALTER TABLE course_exams FORCE ROW LEVEL SECURITY;
ALTER TABLE course_meetings FORCE ROW LEVEL SECURITY;
ALTER TABLE course_schedules FORCE ROW LEVEL SECURITY;
ALTER TABLE deleted_blocks FORCE ROW LEVEL SECURITY;
ALTER TABLE deleted_containers FORCE ROW LEVEL SECURITY;
ALTER TABLE diary_days FORCE ROW LEVEL SECURITY;
ALTER TABLE exam_scores FORCE ROW LEVEL SECURITY;
ALTER TABLE file_refs FORCE ROW LEVEL SECURITY;
ALTER TABLE file_versions FORCE ROW LEVEL SECURITY;
ALTER TABLE files FORCE ROW LEVEL SECURITY;
ALTER TABLE invitations FORCE ROW LEVEL SECURITY;
ALTER TABLE loose_objects FORCE ROW LEVEL SECURITY;
ALTER TABLE meeting_attendance FORCE ROW LEVEL SECURITY;
ALTER TABLE notification_outbox FORCE ROW LEVEL SECURITY;
ALTER TABLE notifications FORCE ROW LEVEL SECURITY;
ALTER TABLE oidc_former_providers FORCE ROW LEVEL SECURITY;
ALTER TABLE oidc_held_sign_ins FORCE ROW LEVEL SECURITY;
ALTER TABLE oidc_login_flows FORCE ROW LEVEL SECURITY;
ALTER TABLE oidc_pending_providers FORCE ROW LEVEL SECURITY;
ALTER TABLE oidc_providers FORCE ROW LEVEL SECURITY;
ALTER TABLE organization_members FORCE ROW LEVEL SECURITY;
ALTER TABLE organizations FORCE ROW LEVEL SECURITY;
ALTER TABLE public_links FORCE ROW LEVEL SECURITY;
ALTER TABLE search_outbox FORCE ROW LEVEL SECURITY;
ALTER TABLE subscriptions FORCE ROW LEVEL SECURITY;
ALTER TABLE team_members FORCE ROW LEVEL SECURITY;
ALTER TABLE teams FORCE ROW LEVEL SECURITY;
ALTER TABLE time_entries FORCE ROW LEVEL SECURITY;
ALTER TABLE unit_notes FORCE ROW LEVEL SECURITY;
ALTER TABLE unit_progress FORCE ROW LEVEL SECURITY;
ALTER TABLE unit_reviews FORCE ROW LEVEL SECURITY;
ALTER TABLE upload_parts FORCE ROW LEVEL SECURITY;
ALTER TABLE uploads FORCE ROW LEVEL SECURITY;
ALTER TABLE workspace_members FORCE ROW LEVEL SECURITY;
ALTER TABLE workspaces FORCE ROW LEVEL SECURITY;

CREATE POLICY org_templates ON blocks FOR SELECT USING (((org_id = '00000000-0000-7000-8000-00000000a000'::uuid) AND (( SELECT NULLIF(current_setting('fvoci.org'::text, true), ''::text) AS "nullif") IS NOT NULL)));

CREATE POLICY org_templates ON containers FOR SELECT USING (((org_id = '00000000-0000-7000-8000-00000000a000'::uuid) AND (( SELECT NULLIF(current_setting('fvoci.org'::text, true), ''::text) AS "nullif") IS NOT NULL)));

CREATE POLICY org_templates ON files FOR SELECT USING (((org_id = '00000000-0000-7000-8000-00000000a000'::uuid) AND (( SELECT NULLIF(current_setting('fvoci.org'::text, true), ''::text) AS "nullif") IS NOT NULL)));

CREATE POLICY org_wall ON api_token_workspaces USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON audit_log USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON blocks USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON body_docs USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON body_links USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON body_updates USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON body_version_editors USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON body_versions USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON card_assignees USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON card_due_notices USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON categories USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON category_containers USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON comment_versions USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON container_members USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON container_teams USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON containers USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON course_exams USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON course_meetings USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON course_schedules USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON deleted_blocks USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON deleted_containers USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON diary_days USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON exam_scores USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON file_refs USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON file_versions USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON files USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON invitations USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON loose_objects USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON meeting_attendance USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON notification_outbox USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON notifications USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON oidc_former_providers USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON oidc_held_sign_ins USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON oidc_login_flows USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON oidc_pending_providers USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON oidc_providers USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON organization_members USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON organizations USING (((id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON public_links USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON search_outbox USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON subscriptions USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON team_members USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON teams USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON time_entries USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON unit_notes USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON unit_progress USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON unit_reviews USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON upload_parts USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON uploads USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON workspace_members USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));

CREATE POLICY org_wall ON workspaces USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));
