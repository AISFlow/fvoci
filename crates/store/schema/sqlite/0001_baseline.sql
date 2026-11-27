-- The schema of a SQLite database: one organization, the system's and what the first
-- account starts, held so by triggers (SQLite has no row-level security). The tables
-- come first, then the rows every database starts with, the indexes and the triggers.
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

CREATE TABLE users (
    id TEXT NOT NULL PRIMARY KEY,
    username TEXT NOT NULL,
    email TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    is_admin BOOLEAN NOT NULL DEFAULT FALSE,
    display_name TEXT NOT NULL DEFAULT '',
    first_name TEXT NOT NULL DEFAULT '',
    last_name TEXT NOT NULL DEFAULT '',
    latin_first_name TEXT NOT NULL DEFAULT '',
    latin_last_name TEXT NOT NULL DEFAULT '',
    custom_name TEXT NOT NULL DEFAULT '',
    name_shown TEXT NOT NULL DEFAULT 'custom_name',
    title TEXT NOT NULL DEFAULT '',
    credentials TEXT NOT NULL DEFAULT '',
    welcomed_at BIGINT,
    password_locked_at BIGINT NOT NULL DEFAULT 0,
    deleted_at BIGINT CHECK (deleted_at IS NULL OR deleted_at > 0),
    delete_after BIGINT NOT NULL DEFAULT 0 CHECK (delete_after = 0 OR (deleted_at IS NOT NULL AND deleted_at > 0)),
    erase_started_at BIGINT NOT NULL DEFAULT 0 CHECK (erase_started_at = 0 OR (deleted_at IS NOT NULL AND deleted_at > 0)),
    erased_at BIGINT NOT NULL DEFAULT 0 CHECK (erased_at = 0 OR erase_started_at > 0),
    email_verified_at BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE instance (
    id SMALLINT NOT NULL PRIMARY KEY CHECK (id = 1),
    bootstrapped BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE sessions (
    id TEXT NOT NULL PRIMARY KEY,
    token_hash TEXT NOT NULL,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at BIGINT NOT NULL,
    last_used_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    user_agent TEXT NOT NULL,
    login_provider_id TEXT,
    login_org_id TEXT,
    login_issuer TEXT,
    login_revision BIGINT,
    login_subject TEXT,
    password_confirmed_at BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE containers (
    id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES "workspaces" (id),
    created_by TEXT REFERENCES users (id),
    modified_by TEXT REFERENCES users (id),
    minimum_role TEXT NOT NULL DEFAULT 'editor' CHECK (minimum_role IN ('viewer', 'commenter', 'editor')),
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    icon TEXT NOT NULL,
    show_description BOOLEAN NOT NULL,
    is_template BOOLEAN NOT NULL,
    is_builtin BOOLEAN NOT NULL DEFAULT false,
    template_version BIGINT NOT NULL DEFAULT 0,
    properties TEXT NOT NULL,
    card_properties TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    next_card_number BIGINT NOT NULL DEFAULT 1,
    kind TEXT NOT NULL DEFAULT 'board' CHECK (kind IN ('board', 'wiki', 'diary', 'course')),
    visibility TEXT NOT NULL DEFAULT 'private' CHECK (visibility IN ('open', 'private')),
    due_date_property_id TEXT,
    done_when TEXT
);

CREATE TABLE blocks (
    id TEXT NOT NULL PRIMARY KEY,
    container_id TEXT NOT NULL REFERENCES "containers" (id),
    parent_id TEXT,
    created_by TEXT REFERENCES users (id),
    modified_by TEXT REFERENCES users (id),
    type TEXT NOT NULL,
    title TEXT NOT NULL,
    fields TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    body TEXT,
    body_version BIGINT,
    body_text TEXT,
    body_tasks_total BIGINT NOT NULL DEFAULT 0,
    body_tasks_done BIGINT NOT NULL DEFAULT 0,
    number BIGINT,
    body_first_image TEXT,
    due_at TEXT,
    due_millis BIGINT,
    is_done BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE (container_id, id),
    FOREIGN KEY (container_id, parent_id) REFERENCES blocks (container_id, id)
);

CREATE TABLE deleted_containers (
    id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    deleted_at BIGINT NOT NULL,
    deleted_by TEXT,
    deletion_id TEXT NOT NULL,
    container TEXT NOT NULL,
    members TEXT NOT NULL,
    files TEXT NOT NULL,
    places TEXT NOT NULL DEFAULT '[]',
    team_roles TEXT NOT NULL DEFAULT '[]',
    next_card_number BIGINT NOT NULL DEFAULT 1
);

CREATE TABLE deleted_blocks (
    id TEXT NOT NULL PRIMARY KEY,
    container_id TEXT NOT NULL,
    parent_id TEXT,
    deleted_at BIGINT NOT NULL,
    deleted_by TEXT,
    deletion_id TEXT NOT NULL,
    block TEXT NOT NULL,
    block_type TEXT NOT NULL DEFAULT ''
);

CREATE TABLE categories (
    id TEXT NOT NULL PRIMARY KEY,
    name TEXT NOT NULL,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL REFERENCES "workspaces" (id),
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    collapsed BOOLEAN NOT NULL DEFAULT false,
    type TEXT NOT NULL CHECK (type IN ('system', 'custom')),
    sort_order BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE category_containers (
    id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    category_id TEXT NOT NULL REFERENCES categories (id) ON DELETE CASCADE,
    container_id TEXT NOT NULL REFERENCES "containers" (id) ON DELETE CASCADE,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    sort_order BIGINT NOT NULL DEFAULT 0,
    hidden BOOLEAN NOT NULL DEFAULT false,
    UNIQUE (user_id, container_id)
);

CREATE TABLE files (
    id TEXT NOT NULL PRIMARY KEY,
    container_id TEXT NOT NULL REFERENCES "containers" (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    original_name TEXT NOT NULL,
    extension TEXT NOT NULL,
    size BIGINT NOT NULL,
    path TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    unreferenced_at BIGINT,
    version BIGINT NOT NULL DEFAULT 1,
    created_by TEXT,
    UNIQUE (container_id, name)
);

CREATE TABLE public_links (
    container_id TEXT NOT NULL PRIMARY KEY REFERENCES "containers" (id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL,
    token TEXT UNIQUE,
    modified_by TEXT REFERENCES users (id) ON DELETE SET NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE subscriptions (
    block_type TEXT NOT NULL,
    block_id TEXT NOT NULL REFERENCES blocks (id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    notified_at BIGINT NOT NULL DEFAULT 0,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (block_id, user_id)
);

CREATE TABLE file_refs (
    block_id TEXT NOT NULL,
    container_id TEXT NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (block_id, name)
);

CREATE TABLE data_steps (
    name TEXT NOT NULL PRIMARY KEY
);

CREATE TABLE uploads (
    id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    container_id TEXT NOT NULL,
    name TEXT NOT NULL,
    size BIGINT NOT NULL,
    content_type TEXT NOT NULL,
    part_size BIGINT NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    closed_at BIGINT NOT NULL,
    multipart_id TEXT NOT NULL,
    path TEXT NOT NULL DEFAULT '',
    purpose TEXT NOT NULL DEFAULT 'file'
);

CREATE TABLE upload_parts (
    upload_id TEXT NOT NULL REFERENCES uploads (id) ON DELETE CASCADE,
    number BIGINT NOT NULL,
    size BIGINT NOT NULL,
    part_id TEXT NOT NULL,
    PRIMARY KEY (upload_id, number)
);

CREATE TABLE comment_versions (
    block_id TEXT NOT NULL,
    version BIGINT NOT NULL,
    body TEXT,
    files TEXT NOT NULL,
    written_by TEXT,
    written_at BIGINT NOT NULL,
    PRIMARY KEY (block_id, version)
);

CREATE TABLE organizations (
    id TEXT NOT NULL PRIMARY KEY,
    name TEXT NOT NULL,
    is_system BOOLEAN NOT NULL DEFAULT false,
    default_workspace_id TEXT REFERENCES "workspaces" (id),
    password_login BOOLEAN NOT NULL DEFAULT true,
    settings TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    public_links BOOLEAN NOT NULL DEFAULT true,
    created_by TEXT REFERENCES users (id) ON DELETE SET NULL,
    deleted_at BIGINT CHECK (deleted_at IS NULL OR deleted_at > 0),
    kind TEXT NOT NULL DEFAULT 'shared' CHECK (kind IN ('shared', 'personal')),
    owner_id TEXT REFERENCES users (id) CHECK ((kind = 'personal') = (owner_id IS NOT NULL) AND (NOT is_system OR kind = 'shared')),
    allow_copy_out BOOLEAN NOT NULL DEFAULT TRUE,
    allow_api_tokens BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE organization_members (
    org_id TEXT NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('admin', 'member')),
    created_at BIGINT NOT NULL,
    PRIMARY KEY (org_id, user_id)
);

CREATE TABLE workspace_members (
    workspace_id TEXT NOT NULL REFERENCES "workspaces" (id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('admin', 'member', 'guest')),
    via_org_admin BOOLEAN NOT NULL DEFAULT false,
    prior_role TEXT CHECK (prior_role IN ('admin', 'member', 'guest')),
    created_at BIGINT NOT NULL,
    prior_container_roles TEXT,
    PRIMARY KEY (workspace_id, user_id)
);

CREATE TABLE workspaces (
    id TEXT NOT NULL PRIMARY KEY,
    settings TEXT NOT NULL,
    modified_by TEXT REFERENCES users (id) ON DELETE SET NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    org_id TEXT NOT NULL REFERENCES organizations (id),
    name TEXT NOT NULL DEFAULT '',
    is_system BOOLEAN NOT NULL DEFAULT false,
    guest_invitations BOOLEAN NOT NULL DEFAULT true,
    deleted_at BIGINT CHECK (deleted_at IS NULL OR deleted_at > 0),
    welcomed_at BIGINT CHECK (welcomed_at IS NULL OR welcomed_at > 0)
);

CREATE TABLE loose_objects (
    path TEXT NOT NULL PRIMARY KEY,
    state TEXT NOT NULL CHECK (state IN ('writing', 'doomed')),
    created_at BIGINT NOT NULL,
    retry_at BIGINT NOT NULL,
    attempts BIGINT NOT NULL DEFAULT 0,
    last_attempt_at BIGINT NOT NULL DEFAULT 0,
    last_error TEXT NOT NULL DEFAULT ''
);

CREATE TABLE oidc_providers (
    id TEXT NOT NULL PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    issuer TEXT NOT NULL,
    client_id TEXT NOT NULL,
    client_secret TEXT NOT NULL,
    auto_join BOOLEAN NOT NULL DEFAULT true,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    sso_required BOOLEAN NOT NULL DEFAULT false,
    revision BIGINT NOT NULL DEFAULT 1
);

CREATE TABLE oidc_login_flows (
    state_hash TEXT NOT NULL PRIMARY KEY,
    browser_hash TEXT NOT NULL,
    nonce TEXT NOT NULL,
    pkce_verifier TEXT NOT NULL,
    org_id TEXT NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    provider_id TEXT NOT NULL REFERENCES oidc_providers (id) ON DELETE CASCADE,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    link_session_id TEXT,
    pending_at BIGINT,
    pending_by TEXT,
    held_hash TEXT
);

CREATE TABLE user_identities (
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    email TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (issuer, subject)
);

CREATE TABLE body_docs (
    block_id TEXT NOT NULL PRIMARY KEY,
    state BLOB NOT NULL,
    state_seq BIGINT NOT NULL,
    log_end BIGINT NOT NULL,
    projected_seq BIGINT NOT NULL,
    body_digest BLOB NOT NULL,
    epoch BIGINT NOT NULL,
    last_writer TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    agent_token_id TEXT,
    agent_token_name TEXT
);

CREATE TABLE body_updates (
    block_id TEXT NOT NULL REFERENCES body_docs (block_id) ON DELETE CASCADE,
    seq BIGINT NOT NULL,
    bytes BLOB NOT NULL,
    written_by TEXT,
    written_at BIGINT NOT NULL,
    actor_token_id TEXT,
    PRIMARY KEY (block_id, seq)
);

CREATE TABLE invitations (
    id TEXT NOT NULL PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('member', 'guest')),
    container_id TEXT REFERENCES "containers" (id) ON DELETE CASCADE,
    container_role TEXT CHECK (container_role IN ('viewer', 'commenter', 'editor')),
    token_hash TEXT NOT NULL,
    created_by TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    max_uses BIGINT CHECK (max_uses > 0),
    uses BIGINT NOT NULL DEFAULT 0,
    revoked_at BIGINT NOT NULL DEFAULT 0,
    parent_id TEXT REFERENCES invitations (id) ON DELETE CASCADE,
    recipient TEXT,
    CHECK ((role = 'guest') = (container_id IS NOT NULL)),
    CHECK ((role = 'guest') = (container_role IS NOT NULL))
);

CREATE TABLE password_resets (
    token_hash TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    expires_at BIGINT NOT NULL
);

CREATE TABLE teams (
    id TEXT NOT NULL,
    org_id TEXT NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    name TEXT NOT NULL CHECK (length(name) > 0),
    name_key TEXT NOT NULL CHECK (length(name_key) > 0),
    PRIMARY KEY (id, org_id),
    UNIQUE (id, workspace_id, org_id),
    UNIQUE (workspace_id, name_key, org_id),
    FOREIGN KEY (workspace_id, org_id) REFERENCES workspaces (id, org_id) ON DELETE CASCADE
);

CREATE TABLE team_members (
    team_id TEXT NOT NULL,
    org_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    PRIMARY KEY (team_id, user_id, org_id),
    FOREIGN KEY (team_id, workspace_id, org_id) REFERENCES teams (id, workspace_id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, user_id) REFERENCES workspace_members (workspace_id, user_id) ON DELETE CASCADE
);

CREATE TABLE container_teams (
    container_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    org_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('viewer', 'commenter', 'editor')),
    PRIMARY KEY (container_id, team_id, org_id),
    FOREIGN KEY (container_id, workspace_id) REFERENCES "containers" (id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY (team_id, workspace_id, org_id) REFERENCES teams (id, workspace_id, org_id) ON DELETE CASCADE
);

CREATE TABLE oidc_pending_providers (
    org_id TEXT NOT NULL PRIMARY KEY REFERENCES organizations (id) ON DELETE CASCADE,
    provider_id TEXT NOT NULL REFERENCES oidc_providers (id) ON DELETE CASCADE,
    issuer TEXT NOT NULL,
    client_id TEXT NOT NULL,
    client_secret TEXT NOT NULL,
    created_at BIGINT NOT NULL
);

CREATE TABLE oidc_former_providers (
    org_id TEXT NOT NULL PRIMARY KEY REFERENCES organizations (id) ON DELETE CASCADE,
    issuer TEXT NOT NULL,
    client_id TEXT NOT NULL,
    client_secret TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL
);

CREATE TABLE oidc_held_sign_ins (
    held_hash TEXT NOT NULL PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    provider_id TEXT NOT NULL REFERENCES oidc_providers (id) ON DELETE CASCADE,
    revision BIGINT NOT NULL,
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    email TEXT NOT NULL,
    email_unverified BOOLEAN NOT NULL,
    name TEXT NOT NULL,
    preferred_username TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL
);

CREATE TABLE body_version_editors (
    block_id TEXT NOT NULL,
    version BIGINT NOT NULL,
    user_id TEXT NOT NULL,
    PRIMARY KEY (block_id, version, user_id)
);

CREATE TABLE body_links (
    source_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    container_id TEXT NOT NULL,
    PRIMARY KEY (source_id, target_id)
);

CREATE TABLE search_index_state (
    id SMALLINT NOT NULL PRIMARY KEY CHECK (id = 1),
    name TEXT NOT NULL,
    uuid TEXT NOT NULL,
    version BIGINT NOT NULL
);

CREATE TABLE signups (
    id TEXT NOT NULL PRIMARY KEY,
    username TEXT NOT NULL,
    email TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '',
    password_hash TEXT NOT NULL,
    language TEXT NOT NULL DEFAULT '',
    organization_name TEXT,
    invitation_id TEXT REFERENCES invitations (id) ON DELETE CASCADE,
    invitation_org_id TEXT,
    token_hash TEXT,
    token_expires_at BIGINT,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    browser_hash TEXT,
    CHECK ((organization_name IS NULL) <> (invitation_id IS NULL)),
    CHECK ((invitation_id IS NULL) = (invitation_org_id IS NULL)),
    CHECK ((token_hash IS NULL) = (token_expires_at IS NULL))
);

CREATE TABLE search_outbox (
    scope TEXT NOT NULL CHECK (scope IN ('block', 'container')),
    id TEXT NOT NULL,
    org_id TEXT NOT NULL,
    seq BIGINT NOT NULL,
    due_at BIGINT NOT NULL,
    attempts BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (org_id, scope, id)
);

CREATE TABLE file_versions (
    file_id TEXT NOT NULL,
    version BIGINT NOT NULL,
    path TEXT NOT NULL,
    size BIGINT NOT NULL,
    created_at BIGINT NOT NULL,
    created_by TEXT,
    restored_from BIGINT,
    PRIMARY KEY (file_id, version)
);

CREATE TABLE email_changes (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    new_email TEXT NOT NULL,
    token_hash TEXT,
    token_expires_at BIGINT,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    CHECK ((token_hash IS NULL) = (token_expires_at IS NULL))
);

CREATE TABLE diary_days (
    container_id TEXT NOT NULL,
    day TEXT NOT NULL,
    month_day TEXT NOT NULL,
    entry_id TEXT NOT NULL UNIQUE REFERENCES blocks (id) ON DELETE CASCADE,
    mood TEXT,
    PRIMARY KEY (container_id, day)
);

CREATE TABLE notifications (
    id TEXT NOT NULL,
    org_id TEXT NOT NULL,
    recipient_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    block_id TEXT NOT NULL,
    container_id TEXT NOT NULL,
    actor_id TEXT,
    created_at BIGINT NOT NULL,
    read_at BIGINT NOT NULL DEFAULT 0,
    made_by TEXT,
    updated_at BIGINT NOT NULL DEFAULT 0,
    times BIGINT NOT NULL DEFAULT 1,
    doers TEXT NOT NULL DEFAULT '[]',
    counted TEXT NOT NULL DEFAULT '[]',
    kind TEXT NOT NULL DEFAULT 'mention' CHECK (kind IN ( 'mention', 'assignment', 'comment', 'review_due', 'meeting_upcoming', 'due_soon', 'added_to_container')),
    actor_token_name TEXT,
    PRIMARY KEY (org_id, id)
);

CREATE TABLE email_reverts (
    token_hash TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    email TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL
);

CREATE TABLE unit_progress (
    unit_id TEXT NOT NULL,
    container_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    completed_at INTEGER,
    planned_date TEXT,
    last_position TEXT,
    updated_at INTEGER NOT NULL,
    org_id TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (unit_id, user_id)
);

CREATE TABLE time_entries (
    id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL DEFAULT '',
    user_id TEXT NOT NULL,
    workspace_id TEXT,
    container_id TEXT,
    block_id TEXT,
    target_kind TEXT CHECK (target_kind IS NULL OR target_kind IN ('card', 'unit', 'board', 'course')),
    target_title TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('lecture', 'study', 'work', 'other')),
    started_at BIGINT NOT NULL,
    ended_at BIGINT,
    note TEXT NOT NULL DEFAULT '',
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    CHECK (ended_at IS NULL OR ended_at >= started_at)
);

CREATE TABLE course_schedules (
    container_id TEXT NOT NULL,
    start_date TEXT NOT NULL,
    target_date TEXT NOT NULL,
    study_days INTEGER NOT NULL CHECK (study_days >= 1 AND study_days <= 127),
    updated_at INTEGER NOT NULL,
    org_id TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (container_id, org_id)
);

CREATE TABLE unit_reviews (
    id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL DEFAULT '',
    unit_id TEXT NOT NULL,
    container_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    scheduled_date TEXT NOT NULL,
    completed_at BIGINT,
    interval_days INTEGER NOT NULL CHECK (interval_days >= 1 AND interval_days <= 3650),
    notified_at BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE unit_notes (
    id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL DEFAULT '',
    unit_id TEXT NOT NULL,
    container_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('question', 'mistake', 'review')),
    content TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE course_meetings (
    id TEXT NOT NULL,
    org_id TEXT NOT NULL DEFAULT '',
    container_id TEXT NOT NULL,
    title TEXT NOT NULL,
    starts_at BIGINT NOT NULL,
    location TEXT NOT NULL DEFAULT '',
    unit_ids TEXT NOT NULL DEFAULT '[]',
    presenter_id TEXT,
    host_id TEXT,
    notes TEXT NOT NULL DEFAULT '',
    notified_at BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (org_id, id)
);

CREATE TABLE meeting_attendance (
    meeting_id TEXT NOT NULL,
    org_id TEXT NOT NULL DEFAULT '',
    user_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('present', 'late', 'absent', 'excused')),
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (meeting_id, org_id, user_id)
);

CREATE TABLE course_exams (
    id TEXT NOT NULL,
    org_id TEXT NOT NULL DEFAULT '',
    container_id TEXT NOT NULL,
    title TEXT NOT NULL,
    exam_date TEXT,
    max_score REAL NOT NULL CHECK (max_score > 0 AND max_score <= 1000000000),
    description TEXT NOT NULL DEFAULT '',
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    results_published_at BIGINT,
    published_average_score REAL,
    published_score_bucket BIGINT,
    published_submission_count BIGINT,
    PRIMARY KEY (org_id, id)
);

CREATE TABLE exam_scores (
    exam_id TEXT NOT NULL,
    org_id TEXT NOT NULL DEFAULT '',
    container_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    score REAL NOT NULL CHECK (score >= 0 AND score <= 1000000000),
    note TEXT NOT NULL DEFAULT '',
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (exam_id, org_id, user_id)
);

CREATE TABLE container_members (
    container_id TEXT NOT NULL REFERENCES containers (id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('admin', 'editor', 'commenter', 'viewer')),
    PRIMARY KEY (container_id, user_id)
);

CREATE TABLE user_settings (
    user_id TEXT NOT NULL PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    language TEXT NOT NULL DEFAULT 'auto',
    theme TEXT NOT NULL DEFAULT 'auto' CHECK (theme IN ('auto', 'light', 'dark')),
    density TEXT NOT NULL DEFAULT 'auto' CHECK (density IN ('auto', 'compact', 'comfortable')),
    card_open TEXT NOT NULL DEFAULT 'center' CHECK (card_open IN ('center', 'side', 'full')),
    haptics BOOLEAN NOT NULL DEFAULT TRUE,
    week_start TEXT NOT NULL DEFAULT 'auto' CHECK (week_start IN ('auto', 'sunday', 'monday', 'saturday')),
    hour_cycle TEXT NOT NULL DEFAULT 'auto' CHECK (hour_cycle IN ('auto', 'h12', 'h23')),
    time_zone TEXT NOT NULL DEFAULT 'auto',
    notify_mentions BOOLEAN NOT NULL DEFAULT TRUE,
    notify_assignments BOOLEAN NOT NULL DEFAULT TRUE,
    notify_comments BOOLEAN NOT NULL DEFAULT TRUE,
    notify_reviews BOOLEAN NOT NULL DEFAULT TRUE,
    notify_meetings BOOLEAN NOT NULL DEFAULT TRUE,
    notify_due BOOLEAN NOT NULL DEFAULT TRUE
);

CREATE TABLE user_client_state (
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (user_id, key)
);

CREATE TABLE card_assignees (
    org_id TEXT NOT NULL DEFAULT '',
    container_id TEXT NOT NULL REFERENCES containers (id) ON DELETE CASCADE,
    card_id TEXT NOT NULL REFERENCES blocks (id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (org_id, card_id, user_id)
);

CREATE TABLE card_due_notices (
    org_id TEXT NOT NULL DEFAULT '',
    card_id TEXT NOT NULL,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    due_key TEXT NOT NULL,
    notified_at BIGINT NOT NULL,
    PRIMARY KEY (org_id, card_id, user_id, due_key)
);

CREATE TABLE notification_outbox (
    org_id TEXT NOT NULL,
    recipient_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ( 'mention', 'assignment', 'comment', 'review_due', 'meeting_upcoming', 'due_soon', 'added_to_container')),
    source_id TEXT NOT NULL,
    block_id TEXT NOT NULL,
    container_id TEXT NOT NULL,
    actor_id TEXT,
    seq BIGINT NOT NULL,
    due_at BIGINT NOT NULL,
    attempts BIGINT NOT NULL DEFAULT 0,
    actor_token_name TEXT,
    PRIMARY KEY (org_id, recipient_id, kind, source_id)
);

CREATE TABLE api_tokens (
    id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
    token_prefix TEXT NOT NULL,
    token_hash TEXT NOT NULL,
    scope TEXT NOT NULL CHECK (scope IN ('read', 'write')),
    allow_personal_space BOOLEAN NOT NULL DEFAULT FALSE,
    notify_self BOOLEAN NOT NULL DEFAULT TRUE,
    origin_provider_id TEXT,
    origin_provider_revision BIGINT,
    origin_issuer TEXT,
    origin_subject TEXT,
    last_used_at BIGINT,
    last_used_address TEXT,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL CHECK (expires_at > created_at),
    revoked_at BIGINT,
    CHECK ((origin_provider_id IS NULL) = (origin_provider_revision IS NULL)),
    CHECK ((origin_provider_id IS NULL) = (origin_issuer IS NULL))
);

CREATE TABLE api_token_workspaces (
    org_id TEXT NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    token_id TEXT NOT NULL REFERENCES api_tokens (id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    PRIMARY KEY (token_id, workspace_id),
    FOREIGN KEY (workspace_id, org_id) REFERENCES workspaces (id, org_id) ON DELETE CASCADE
);

CREATE TABLE audit_log (
    id TEXT NOT NULL PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    workspace_id TEXT REFERENCES workspaces (id) ON DELETE CASCADE,
    actor_id TEXT,
    actor_kind TEXT NOT NULL DEFAULT 'person' CHECK (actor_kind IN ('person', 'server_operator', 'server')),
    action TEXT NOT NULL CHECK (action IN ( 'self_added_to_container', 'added_to_container', 'org_admin_joined', 'org_admin_left', 'org_admin_revoked', 'org_admin_granted', 'org_admin_removed', 'org_member_removed', 'workspace_created', 'workspace_renamed', 'workspace_deleted', 'workspace_restored', 'public_links_disabled', 'public_links_enabled', 'container_copied_out', 'copy_out_disabled', 'copy_out_enabled', 'api_tokens_enabled', 'api_tokens_disabled', 'api_token_revoked', 'oidc_provider_configured', 'oidc_provider_removed', 'sso_required', 'sso_released', 'oidc_provider_pending_saved', 'oidc_provider_pending_applied', 'oidc_provider_pending_discarded', 'password_reset_link_made', 'password_set_by_operator', 'oidc_former_provider_kept', 'oidc_former_provider_ended', 'oidc_former_provider_lapsed', 'oidc_identity_bound', 'oidc_identity_carried_over', 'email_verified_by_operator')),
    target_id TEXT NOT NULL,
    subject_id TEXT,
    created_at BIGINT NOT NULL,
    detail TEXT,
    identity TEXT,
    CHECK ((actor_kind = 'person' AND actor_id IS NOT NULL) OR (actor_kind IN ('server_operator', 'server') AND actor_id IS NULL)),
    CHECK (workspace_id IS NOT NULL OR action IN ( 'org_admin_granted', 'org_admin_removed', 'org_member_removed', 'workspace_created', 'workspace_renamed', 'workspace_deleted', 'workspace_restored', 'public_links_disabled', 'public_links_enabled', 'container_copied_out', 'copy_out_disabled', 'copy_out_enabled', 'api_tokens_enabled', 'api_tokens_disabled', 'api_token_revoked', 'oidc_provider_configured', 'oidc_provider_removed', 'sso_required', 'sso_released', 'oidc_provider_pending_saved', 'oidc_provider_pending_applied', 'oidc_provider_pending_discarded', 'password_reset_link_made', 'password_set_by_operator', 'oidc_former_provider_kept', 'oidc_former_provider_ended', 'oidc_former_provider_lapsed', 'oidc_identity_bound', 'oidc_identity_carried_over', 'email_verified_by_operator'))
);

CREATE TABLE body_versions (
    block_id TEXT NOT NULL,
    version BIGINT NOT NULL,
    body TEXT,
    body_digest BLOB NOT NULL,
    files TEXT NOT NULL,
    kept_at BIGINT NOT NULL,
    body_bytes INTEGER NOT NULL DEFAULT 0 CHECK (body_bytes >= 0),
    cause TEXT NOT NULL DEFAULT 'auto' CHECK (cause IN ('auto', 'manual', 'restored', 'agent')),
    actor_token_id TEXT,
    actor_token_name TEXT,
    PRIMARY KEY (block_id, version)
);

INSERT INTO instance (id, bootstrapped) VALUES (1, FALSE);
INSERT INTO organizations (id, name, is_system, settings, created_at, updated_at)
    VALUES ('00000000-0000-7000-8000-00000000a000', '', TRUE, '{}', 0, 0);
INSERT INTO workspaces (id, settings, created_at, updated_at, org_id, name, is_system)
    VALUES ('00000000-0000-7000-8000-00000000a001', '{}', 0, 0,
            '00000000-0000-7000-8000-00000000a000', '', TRUE);
UPDATE organizations SET default_workspace_id = '00000000-0000-7000-8000-00000000a001'
    WHERE id = '00000000-0000-7000-8000-00000000a000';

CREATE UNIQUE INDEX sessions_token_hash_key ON sessions (token_hash);
CREATE INDEX sessions_user_id_idx ON sessions (user_id);
CREATE INDEX deleted_blocks_deletion_id_idx ON deleted_blocks (deletion_id);
CREATE INDEX deleted_blocks_deleted_at_idx ON deleted_blocks (deleted_at);
CREATE INDEX uploads_user_id_idx ON uploads (user_id);
CREATE INDEX uploads_expires_at_idx ON uploads (expires_at);
CREATE UNIQUE INDEX organizations_system_key ON organizations (is_system) WHERE is_system;
CREATE INDEX organization_members_user_id_idx ON organization_members (user_id);
CREATE INDEX workspace_members_user_id_idx ON workspace_members (user_id);
CREATE INDEX loose_objects_state_retry_at_idx ON loose_objects (state, retry_at);
CREATE INDEX uploads_path_idx ON uploads (path);
CREATE INDEX workspaces_org_id_idx ON workspaces (org_id);
CREATE INDEX categories_user_id_workspace_id_idx ON categories (user_id, workspace_id);
CREATE UNIQUE INDEX oidc_providers_org_id_key ON oidc_providers (org_id);
CREATE INDEX oidc_login_flows_expires_at_idx ON oidc_login_flows (expires_at);
CREATE INDEX user_identities_user_id_idx ON user_identities (user_id);
CREATE UNIQUE INDEX users_email_key ON users (lower(email)) WHERE email <> '';
CREATE INDEX blocks_created_by_idx ON blocks (created_by);
CREATE INDEX blocks_modified_by_idx ON blocks (modified_by);
CREATE UNIQUE INDEX invitations_token_hash_key ON invitations (token_hash);
CREATE INDEX invitations_workspace_id_idx ON invitations (workspace_id, created_at);
CREATE INDEX password_resets_expires_at_idx ON password_resets (expires_at);
CREATE INDEX password_resets_user_id_idx ON password_resets (user_id);
CREATE UNIQUE INDEX workspaces_scope_key ON workspaces (id, org_id);
CREATE INDEX team_members_user_idx ON team_members (user_id, workspace_id, org_id);
CREATE INDEX sessions_login_origin_idx ON sessions (login_org_id, login_issuer)
    WHERE login_issuer IS NOT NULL;
CREATE INDEX invitations_parent_id_idx ON invitations (parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX oidc_former_providers_expires_at_idx ON oidc_former_providers (expires_at);
CREATE INDEX oidc_held_sign_ins_expires_at_idx ON oidc_held_sign_ins (expires_at);
CREATE INDEX body_links_target_idx ON body_links (target_id);
CREATE UNIQUE INDEX signups_username_key ON signups (lower(username));
CREATE UNIQUE INDEX signups_email_key ON signups (lower(email));
CREATE UNIQUE INDEX signups_token_hash_key ON signups (token_hash);
CREATE INDEX signups_expires_at_idx ON signups (expires_at);
CREATE INDEX signups_invitation_idx ON signups (invitation_id, invitation_org_id)
    WHERE invitation_id IS NOT NULL;
CREATE UNIQUE INDEX signups_browser_hash_key ON signups (browser_hash);
CREATE INDEX blocks_container_id_parent_id_idx ON blocks (container_id, parent_id);
CREATE INDEX container_teams_team_idx ON container_teams (team_id, org_id);
CREATE INDEX containers_workspace_id_is_template_idx ON containers (workspace_id, is_template);
CREATE UNIQUE INDEX containers_workspace_scope_key ON containers (id, workspace_id);
CREATE INDEX category_containers_container_id_idx ON category_containers (container_id);
CREATE INDEX category_containers_category_id_idx ON category_containers (category_id);
CREATE INDEX deleted_containers_deleted_at_idx ON deleted_containers (deleted_at);
CREATE INDEX deleted_containers_workspace_id_idx ON deleted_containers (workspace_id, deleted_at);
CREATE INDEX file_refs_container_id_name_idx ON file_refs (container_id, name);
CREATE INDEX search_outbox_due_at_idx ON search_outbox (due_at);
CREATE UNIQUE INDEX users_username_key ON users (lower(username)) WHERE username <> '';
CREATE INDEX file_versions_path_idx ON file_versions (path);
CREATE INDEX organizations_created_by_idx ON organizations (created_by)
    WHERE created_by IS NOT NULL;
CREATE UNIQUE INDEX email_changes_token_hash_key ON email_changes (token_hash);
CREATE INDEX email_changes_expires_at_idx ON email_changes (expires_at);
CREATE INDEX diary_days_month_day_idx ON diary_days (container_id, month_day);
CREATE INDEX notifications_unread_idx ON notifications (recipient_id) WHERE read_at = 0;
CREATE INDEX notifications_id_idx ON notifications (id);
CREATE INDEX notifications_read_at_idx ON notifications (read_at) WHERE read_at > 0;
CREATE INDEX notifications_recipient_idx ON notifications (recipient_id, updated_at DESC, id DESC);
CREATE INDEX notifications_made_by_idx ON notifications (recipient_id, made_by, created_at);
CREATE INDEX notifications_block_idx ON notifications (block_id);
CREATE INDEX notifications_container_idx ON notifications (container_id);
CREATE UNIQUE INDEX blocks_container_id_number_idx ON blocks (container_id, number);
CREATE INDEX email_reverts_user_id_idx ON email_reverts (user_id, created_at);
CREATE INDEX email_reverts_email_idx ON email_reverts (lower(email));
CREATE INDEX email_reverts_expires_at_idx ON email_reverts (expires_at);
CREATE INDEX unit_progress_container_user_idx
    ON unit_progress (container_id, user_id);
CREATE INDEX unit_progress_container_completed_idx
    ON unit_progress (container_id, completed_at);
CREATE INDEX unit_progress_user_idx
    ON unit_progress (user_id);
CREATE UNIQUE INDEX time_entries_one_active_per_user
    ON time_entries (user_id)
    WHERE ended_at IS NULL;
CREATE INDEX time_entries_user_started_idx
    ON time_entries (user_id, started_at DESC, id DESC);
CREATE INDEX time_entries_org_user_started_idx
    ON time_entries (org_id, user_id, started_at DESC);
CREATE INDEX time_entries_user_block_started_idx
    ON time_entries (user_id, block_id, started_at DESC, id DESC)
    WHERE block_id IS NOT NULL;
CREATE INDEX time_entries_user_container_started_idx
    ON time_entries (user_id, container_id, started_at DESC, id DESC)
    WHERE container_id IS NOT NULL;
CREATE INDEX unit_reviews_user_scheduled_idx
    ON unit_reviews (org_id, user_id, scheduled_date);
CREATE INDEX unit_reviews_container_user_unit_idx
    ON unit_reviews (org_id, container_id, user_id, unit_id);
CREATE INDEX unit_reviews_due_notify_idx
    ON unit_reviews (scheduled_date, created_at, id)
    WHERE completed_at IS NULL AND notified_at IS NULL;
CREATE INDEX unit_reviews_user_idx
    ON unit_reviews (user_id);
CREATE INDEX unit_notes_container_user_unit_created_idx
    ON unit_notes (org_id, container_id, user_id, unit_id, created_at DESC);
CREATE INDEX unit_notes_user_idx
    ON unit_notes (user_id);
CREATE INDEX course_meetings_container_idx
    ON course_meetings (org_id, container_id, starts_at, id);
CREATE INDEX course_meetings_upcoming_idx
    ON course_meetings (starts_at) WHERE notified_at IS NULL;
CREATE INDEX meeting_attendance_meeting_idx
    ON meeting_attendance (org_id, meeting_id);
CREATE INDEX meeting_attendance_user_idx
    ON meeting_attendance (user_id);
CREATE INDEX course_exams_container_idx
    ON course_exams (org_id, container_id, exam_date, created_at, id);
CREATE INDEX exam_scores_container_user_idx
    ON exam_scores (org_id, container_id, user_id);
CREATE INDEX exam_scores_user_idx
    ON exam_scores (user_id);
CREATE INDEX subscriptions_user_id_idx ON subscriptions (user_id);
CREATE INDEX files_unreferenced_at_idx ON files (unreferenced_at);
CREATE INDEX files_path_idx ON files (path);
CREATE UNIQUE INDEX workspaces_name_in_use_key ON workspaces (org_id, name)
    WHERE deleted_at IS NULL AND name <> '';
CREATE INDEX workspaces_deleted_idx ON workspaces (deleted_at)
    WHERE deleted_at IS NOT NULL;
CREATE INDEX users_leaving_idx ON users (delete_after)
    WHERE deleted_at IS NOT NULL AND erased_at = 0;
CREATE INDEX container_members_user_id_idx ON container_members (user_id);
CREATE INDEX blocks_parent_type_created_at_idx ON blocks (parent_id, type, created_at);
CREATE UNIQUE INDEX organizations_owner_id_key ON organizations (owner_id)
    WHERE owner_id IS NOT NULL;
CREATE INDEX organizations_of_people_idx ON organizations (created_at, id)
    WHERE kind = 'shared';
CREATE INDEX blocks_container_type_created_at_idx
    ON blocks (container_id, type, created_at, id);
CREATE INDEX blocks_container_titled_updated_at_idx
    ON blocks (container_id, updated_at DESC, id)
    WHERE type IN ('card', 'page', 'unit');
CREATE INDEX deleted_blocks_container_deleted_at_idx
    ON deleted_blocks (container_id, deleted_at DESC, id);
CREATE INDEX deleted_blocks_container_type_deleted_at_idx
    ON deleted_blocks (container_id, block_type, deleted_at DESC, id);
CREATE INDEX comment_versions_written_by_idx ON comment_versions (written_by);
CREATE INDEX body_version_editors_user_id_idx ON body_version_editors (user_id);
CREATE INDEX files_created_by_idx ON files (created_by);
CREATE INDEX file_versions_created_by_idx ON file_versions (created_by);
CREATE INDEX containers_created_by_idx ON containers (created_by);
CREATE INDEX invitations_created_by_idx ON invitations (created_by);
CREATE INDEX blocks_due_at_idx
    ON blocks (container_id, due_at)
    WHERE due_at IS NOT NULL AND is_done = FALSE;
CREATE INDEX card_assignees_user_container_idx
    ON card_assignees (org_id, user_id, container_id);
CREATE INDEX card_assignees_container_idx
    ON card_assignees (container_id);
CREATE INDEX card_assignees_user_idx
    ON card_assignees (user_id);
CREATE INDEX card_due_notices_user_idx
    ON card_due_notices (user_id);
CREATE INDEX card_due_notices_card_idx
    ON card_due_notices (card_id);
CREATE INDEX notification_outbox_due_at_idx ON notification_outbox (due_at);
CREATE INDEX notification_outbox_block_idx ON notification_outbox (block_id);
CREATE INDEX notification_outbox_container_idx ON notification_outbox (container_id);
CREATE INDEX notifications_same_idx
    ON notifications (recipient_id, block_id, kind, created_at);
CREATE UNIQUE INDEX api_tokens_token_hash_key ON api_tokens (token_hash);
CREATE INDEX api_tokens_user_id_idx ON api_tokens (user_id);
CREATE INDEX api_tokens_origin_provider_idx
    ON api_tokens (origin_provider_id) WHERE origin_provider_id IS NOT NULL;
CREATE INDEX api_token_workspaces_workspace_idx ON api_token_workspaces (org_id, workspace_id);
CREATE INDEX audit_log_workspace_id_idx ON audit_log (workspace_id, created_at);
CREATE INDEX audit_log_org_idx ON audit_log (org_id, created_at) WHERE workspace_id IS NULL;
CREATE INDEX audit_log_actor_identity_idx
    ON audit_log (actor_id) WHERE identity IS NOT NULL;
CREATE INDEX audit_log_subject_identity_idx
    ON audit_log (subject_id) WHERE identity IS NOT NULL;

CREATE TRIGGER organizations_stay_what_they_are BEFORE UPDATE OF is_system ON organizations
WHEN NEW.is_system <> OLD.is_system
BEGIN
    SELECT RAISE(ABORT, 'an organization does not become or stop being the system''s');
END;

CREATE TRIGGER sessions_login_origin_insert BEFORE INSERT ON sessions
WHEN NOT ((NEW.login_provider_id IS NULL AND NEW.login_org_id IS NULL
           AND NEW.login_issuer IS NULL)
       OR (NEW.login_provider_id IS NOT NULL AND NEW.login_org_id IS NOT NULL
           AND NEW.login_issuer IS NOT NULL))
BEGIN
    SELECT RAISE(ABORT, 'a session names its provider, organization and issuer together');
END;

CREATE TRIGGER sessions_login_origin_update
BEFORE UPDATE OF login_provider_id, login_org_id, login_issuer ON sessions
WHEN NOT ((NEW.login_provider_id IS NULL AND NEW.login_org_id IS NULL
           AND NEW.login_issuer IS NULL)
       OR (NEW.login_provider_id IS NOT NULL AND NEW.login_org_id IS NOT NULL
           AND NEW.login_issuer IS NOT NULL))
BEGIN
    SELECT RAISE(ABORT, 'a session names its provider, organization and issuer together');
END;

CREATE TRIGGER sessions_login_subject_insert BEFORE INSERT ON sessions
WHEN (NEW.login_subject IS NOT NULL OR NEW.login_revision IS NOT NULL)
    AND NEW.login_issuer IS NULL
BEGIN
    SELECT RAISE(ABORT, 'only a session made through a provider names a subject or a revision');
END;

CREATE TRIGGER sessions_login_subject_update
BEFORE UPDATE OF login_subject, login_revision, login_issuer ON sessions
WHEN (NEW.login_subject IS NOT NULL OR NEW.login_revision IS NOT NULL)
    AND NEW.login_issuer IS NULL
BEGIN
    SELECT RAISE(ABORT, 'only a session made through a provider names a subject or a revision');
END;

CREATE TRIGGER public_links_of_no_diary BEFORE INSERT ON public_links
WHEN EXISTS (SELECT 1 FROM containers AS c WHERE c.id = NEW.container_id AND c.kind = 'diary')
BEGIN
    SELECT RAISE(ABORT, 'a diary has no link');
END;

CREATE TRIGGER public_links_stay_of_no_diary BEFORE UPDATE OF container_id ON public_links
WHEN EXISTS (SELECT 1 FROM containers AS c WHERE c.id = NEW.container_id AND c.kind = 'diary')
BEGIN
    SELECT RAISE(ABORT, 'a diary has no link');
END;

CREATE TRIGGER organizations_only_one BEFORE INSERT ON organizations
WHEN NOT NEW.is_system AND NEW.kind = 'shared'
     AND EXISTS (SELECT 1 FROM organizations WHERE NOT is_system AND kind = 'shared')
BEGIN
    SELECT RAISE(ABORT, 'an SQLite database holds one organization');
END;

CREATE TRIGGER organizations_keep_their_kind BEFORE UPDATE OF kind, owner_id ON organizations
WHEN NEW.kind IS NOT OLD.kind OR NEW.owner_id IS NOT OLD.owner_id
BEGIN
    SELECT RAISE(ABORT, 'an organization does not change its kind or its owner');
END;

CREATE TRIGGER organization_members_of_a_space BEFORE INSERT ON organization_members
WHEN EXISTS (SELECT 1 FROM organizations AS o
             WHERE o.id = NEW.org_id AND o.kind = 'personal' AND o.owner_id IS NOT NEW.user_id)
BEGIN
    SELECT RAISE(ABORT, 'the space of an account has its owner in it and nobody else');
END;

CREATE TRIGGER organization_members_stay_of_a_space BEFORE UPDATE OF org_id, user_id
    ON organization_members
WHEN EXISTS (SELECT 1 FROM organizations AS o
             WHERE o.id = NEW.org_id AND o.kind = 'personal' AND o.owner_id IS NOT NEW.user_id)
BEGIN
    SELECT RAISE(ABORT, 'the space of an account has its owner in it and nobody else');
END;

CREATE TRIGGER workspace_members_of_a_space BEFORE INSERT ON workspace_members
WHEN EXISTS (SELECT 1 FROM workspaces AS w JOIN organizations AS o ON o.id = w.org_id
             WHERE w.id = NEW.workspace_id AND o.kind = 'personal'
               AND o.owner_id IS NOT NEW.user_id)
BEGIN
    SELECT RAISE(ABORT, 'the space of an account has its owner in it and nobody else');
END;

CREATE TRIGGER workspace_members_stay_of_a_space BEFORE UPDATE OF workspace_id, user_id
    ON workspace_members
WHEN EXISTS (SELECT 1 FROM workspaces AS w JOIN organizations AS o ON o.id = w.org_id
             WHERE w.id = NEW.workspace_id AND o.kind = 'personal'
               AND o.owner_id IS NOT NEW.user_id)
BEGIN
    SELECT RAISE(ABORT, 'the space of an account has its owner in it and nobody else');
END;

CREATE TRIGGER invitations_of_a_space BEFORE INSERT ON invitations
WHEN EXISTS (SELECT 1 FROM organizations AS o
             WHERE o.kind = 'personal'
               AND (o.id = NEW.org_id
                    OR o.id = (SELECT w.org_id FROM workspaces AS w
                               WHERE w.id = NEW.workspace_id)
                    OR o.id = (SELECT w.org_id FROM containers AS c
                               JOIN workspaces AS w ON w.id = c.workspace_id
                               WHERE c.id = NEW.container_id)))
BEGIN
    SELECT RAISE(ABORT, 'nobody is invited into the space of an account');
END;

CREATE TRIGGER invitations_stay_out_of_a_space
    BEFORE UPDATE OF org_id, workspace_id, container_id ON invitations
WHEN EXISTS (SELECT 1 FROM organizations AS o
             WHERE o.kind = 'personal'
               AND (o.id = NEW.org_id
                    OR o.id = (SELECT w.org_id FROM workspaces AS w
                               WHERE w.id = NEW.workspace_id)
                    OR o.id = (SELECT w.org_id FROM containers AS c
                               JOIN workspaces AS w ON w.id = c.workspace_id
                               WHERE c.id = NEW.container_id)))
BEGIN
    SELECT RAISE(ABORT, 'nobody is invited into the space of an account');
END;

CREATE TRIGGER diaries_of_a_space BEFORE INSERT ON containers
WHEN NEW.kind = 'diary'
     AND NOT EXISTS (SELECT 1 FROM workspaces AS w JOIN organizations AS o ON o.id = w.org_id
                     WHERE w.id = NEW.workspace_id AND o.kind = 'personal')
BEGIN
    SELECT RAISE(ABORT, 'a diary is of the space of an account: no other workspace has one');
END;

CREATE TRIGGER diaries_stay_of_a_space BEFORE UPDATE OF workspace_id, kind ON containers
WHEN NEW.kind = 'diary'
     AND NOT EXISTS (SELECT 1 FROM workspaces AS w JOIN organizations AS o ON o.id = w.org_id
                     WHERE w.id = NEW.workspace_id AND o.kind = 'personal')
BEGIN
    SELECT RAISE(ABORT, 'a diary is of the space of an account: no other workspace has one');
END;

CREATE TRIGGER unit_progress_course_of_its_organization
BEFORE INSERT ON unit_progress
WHEN NOT EXISTS (
    SELECT 1 FROM containers AS c
    JOIN workspaces AS w ON w.id = c.workspace_id
    WHERE c.id = NEW.container_id AND c.kind = 'course' AND w.org_id = NEW.org_id
) AND NOT EXISTS (
    SELECT 1 FROM deleted_containers AS dc
    JOIN workspaces AS w ON w.id = dc.workspace_id
    WHERE dc.id = NEW.container_id AND w.org_id = NEW.org_id
      AND CASE
          WHEN json_valid(dc.container) AND json_type(dc.container) = 'object'
              THEN COALESCE(json_extract(dc.container, '$.kind'), 'board') = 'course'
          ELSE 0
      END
)
BEGIN
    SELECT RAISE(ABORT, 'the course of a course record is not a course of its organization');
END;

CREATE TRIGGER unit_progress_stays_of_its_course
BEFORE UPDATE OF container_id, org_id ON unit_progress
WHEN NOT EXISTS (
    SELECT 1 FROM containers AS c
    JOIN workspaces AS w ON w.id = c.workspace_id
    WHERE c.id = NEW.container_id AND c.kind = 'course' AND w.org_id = NEW.org_id
) AND NOT EXISTS (
    SELECT 1 FROM deleted_containers AS dc
    JOIN workspaces AS w ON w.id = dc.workspace_id
    WHERE dc.id = NEW.container_id AND w.org_id = NEW.org_id
      AND CASE
          WHEN json_valid(dc.container) AND json_type(dc.container) = 'object'
              THEN COALESCE(json_extract(dc.container, '$.kind'), 'board') = 'course'
          ELSE 0
      END
)
BEGIN
    SELECT RAISE(ABORT, 'the course of a course record is not a course of its organization');
END;

CREATE TRIGGER unit_reviews_course_of_its_organization
BEFORE INSERT ON unit_reviews
WHEN NOT EXISTS (
    SELECT 1 FROM containers AS c
    JOIN workspaces AS w ON w.id = c.workspace_id
    WHERE c.id = NEW.container_id AND c.kind = 'course' AND w.org_id = NEW.org_id
) AND NOT EXISTS (
    SELECT 1 FROM deleted_containers AS dc
    JOIN workspaces AS w ON w.id = dc.workspace_id
    WHERE dc.id = NEW.container_id AND w.org_id = NEW.org_id
      AND CASE
          WHEN json_valid(dc.container) AND json_type(dc.container) = 'object'
              THEN COALESCE(json_extract(dc.container, '$.kind'), 'board') = 'course'
          ELSE 0
      END
)
BEGIN
    SELECT RAISE(ABORT, 'the course of a course record is not a course of its organization');
END;

CREATE TRIGGER unit_reviews_stays_of_its_course
BEFORE UPDATE OF container_id, org_id ON unit_reviews
WHEN NOT EXISTS (
    SELECT 1 FROM containers AS c
    JOIN workspaces AS w ON w.id = c.workspace_id
    WHERE c.id = NEW.container_id AND c.kind = 'course' AND w.org_id = NEW.org_id
) AND NOT EXISTS (
    SELECT 1 FROM deleted_containers AS dc
    JOIN workspaces AS w ON w.id = dc.workspace_id
    WHERE dc.id = NEW.container_id AND w.org_id = NEW.org_id
      AND CASE
          WHEN json_valid(dc.container) AND json_type(dc.container) = 'object'
              THEN COALESCE(json_extract(dc.container, '$.kind'), 'board') = 'course'
          ELSE 0
      END
)
BEGIN
    SELECT RAISE(ABORT, 'the course of a course record is not a course of its organization');
END;

CREATE TRIGGER unit_notes_course_of_its_organization
BEFORE INSERT ON unit_notes
WHEN NOT EXISTS (
    SELECT 1 FROM containers AS c
    JOIN workspaces AS w ON w.id = c.workspace_id
    WHERE c.id = NEW.container_id AND c.kind = 'course' AND w.org_id = NEW.org_id
) AND NOT EXISTS (
    SELECT 1 FROM deleted_containers AS dc
    JOIN workspaces AS w ON w.id = dc.workspace_id
    WHERE dc.id = NEW.container_id AND w.org_id = NEW.org_id
      AND CASE
          WHEN json_valid(dc.container) AND json_type(dc.container) = 'object'
              THEN COALESCE(json_extract(dc.container, '$.kind'), 'board') = 'course'
          ELSE 0
      END
)
BEGIN
    SELECT RAISE(ABORT, 'the course of a course record is not a course of its organization');
END;

CREATE TRIGGER unit_notes_stays_of_its_course
BEFORE UPDATE OF container_id, org_id ON unit_notes
WHEN NOT EXISTS (
    SELECT 1 FROM containers AS c
    JOIN workspaces AS w ON w.id = c.workspace_id
    WHERE c.id = NEW.container_id AND c.kind = 'course' AND w.org_id = NEW.org_id
) AND NOT EXISTS (
    SELECT 1 FROM deleted_containers AS dc
    JOIN workspaces AS w ON w.id = dc.workspace_id
    WHERE dc.id = NEW.container_id AND w.org_id = NEW.org_id
      AND CASE
          WHEN json_valid(dc.container) AND json_type(dc.container) = 'object'
              THEN COALESCE(json_extract(dc.container, '$.kind'), 'board') = 'course'
          ELSE 0
      END
)
BEGIN
    SELECT RAISE(ABORT, 'the course of a course record is not a course of its organization');
END;

CREATE TRIGGER course_meetings_course_of_its_organization
BEFORE INSERT ON course_meetings
WHEN NOT EXISTS (
    SELECT 1 FROM containers AS c
    JOIN workspaces AS w ON w.id = c.workspace_id
    WHERE c.id = NEW.container_id AND c.kind = 'course' AND w.org_id = NEW.org_id
) AND NOT EXISTS (
    SELECT 1 FROM deleted_containers AS dc
    JOIN workspaces AS w ON w.id = dc.workspace_id
    WHERE dc.id = NEW.container_id AND w.org_id = NEW.org_id
      AND CASE
          WHEN json_valid(dc.container) AND json_type(dc.container) = 'object'
              THEN COALESCE(json_extract(dc.container, '$.kind'), 'board') = 'course'
          ELSE 0
      END
)
BEGIN
    SELECT RAISE(ABORT, 'the course of a course record is not a course of its organization');
END;

CREATE TRIGGER course_meetings_stays_of_its_course
BEFORE UPDATE OF container_id, org_id ON course_meetings
WHEN NOT EXISTS (
    SELECT 1 FROM containers AS c
    JOIN workspaces AS w ON w.id = c.workspace_id
    WHERE c.id = NEW.container_id AND c.kind = 'course' AND w.org_id = NEW.org_id
) AND NOT EXISTS (
    SELECT 1 FROM deleted_containers AS dc
    JOIN workspaces AS w ON w.id = dc.workspace_id
    WHERE dc.id = NEW.container_id AND w.org_id = NEW.org_id
      AND CASE
          WHEN json_valid(dc.container) AND json_type(dc.container) = 'object'
              THEN COALESCE(json_extract(dc.container, '$.kind'), 'board') = 'course'
          ELSE 0
      END
)
BEGIN
    SELECT RAISE(ABORT, 'the course of a course record is not a course of its organization');
END;

CREATE TRIGGER course_exams_course_of_its_organization
BEFORE INSERT ON course_exams
WHEN NOT EXISTS (
    SELECT 1 FROM containers AS c
    JOIN workspaces AS w ON w.id = c.workspace_id
    WHERE c.id = NEW.container_id AND c.kind = 'course' AND w.org_id = NEW.org_id
) AND NOT EXISTS (
    SELECT 1 FROM deleted_containers AS dc
    JOIN workspaces AS w ON w.id = dc.workspace_id
    WHERE dc.id = NEW.container_id AND w.org_id = NEW.org_id
      AND CASE
          WHEN json_valid(dc.container) AND json_type(dc.container) = 'object'
              THEN COALESCE(json_extract(dc.container, '$.kind'), 'board') = 'course'
          ELSE 0
      END
)
BEGIN
    SELECT RAISE(ABORT, 'the course of a course record is not a course of its organization');
END;

CREATE TRIGGER course_exams_stays_of_its_course
BEFORE UPDATE OF container_id, org_id ON course_exams
WHEN NOT EXISTS (
    SELECT 1 FROM containers AS c
    JOIN workspaces AS w ON w.id = c.workspace_id
    WHERE c.id = NEW.container_id AND c.kind = 'course' AND w.org_id = NEW.org_id
) AND NOT EXISTS (
    SELECT 1 FROM deleted_containers AS dc
    JOIN workspaces AS w ON w.id = dc.workspace_id
    WHERE dc.id = NEW.container_id AND w.org_id = NEW.org_id
      AND CASE
          WHEN json_valid(dc.container) AND json_type(dc.container) = 'object'
              THEN COALESCE(json_extract(dc.container, '$.kind'), 'board') = 'course'
          ELSE 0
      END
)
BEGIN
    SELECT RAISE(ABORT, 'the course of a course record is not a course of its organization');
END;

CREATE TRIGGER exam_scores_course_of_its_organization
BEFORE INSERT ON exam_scores
WHEN NOT EXISTS (
    SELECT 1 FROM containers AS c
    JOIN workspaces AS w ON w.id = c.workspace_id
    WHERE c.id = NEW.container_id AND c.kind = 'course' AND w.org_id = NEW.org_id
) AND NOT EXISTS (
    SELECT 1 FROM deleted_containers AS dc
    JOIN workspaces AS w ON w.id = dc.workspace_id
    WHERE dc.id = NEW.container_id AND w.org_id = NEW.org_id
      AND CASE
          WHEN json_valid(dc.container) AND json_type(dc.container) = 'object'
              THEN COALESCE(json_extract(dc.container, '$.kind'), 'board') = 'course'
          ELSE 0
      END
)
BEGIN
    SELECT RAISE(ABORT, 'the course of a course record is not a course of its organization');
END;

CREATE TRIGGER exam_scores_stays_of_its_course
BEFORE UPDATE OF container_id, org_id ON exam_scores
WHEN NOT EXISTS (
    SELECT 1 FROM containers AS c
    JOIN workspaces AS w ON w.id = c.workspace_id
    WHERE c.id = NEW.container_id AND c.kind = 'course' AND w.org_id = NEW.org_id
) AND NOT EXISTS (
    SELECT 1 FROM deleted_containers AS dc
    JOIN workspaces AS w ON w.id = dc.workspace_id
    WHERE dc.id = NEW.container_id AND w.org_id = NEW.org_id
      AND CASE
          WHEN json_valid(dc.container) AND json_type(dc.container) = 'object'
              THEN COALESCE(json_extract(dc.container, '$.kind'), 'board') = 'course'
          ELSE 0
      END
)
BEGIN
    SELECT RAISE(ABORT, 'the course of a course record is not a course of its organization');
END;

CREATE TRIGGER audit_log_unchanged BEFORE UPDATE ON audit_log
WHEN NOT (
    NEW.identity IS NULL
    AND NEW.id IS OLD.id AND NEW.org_id IS OLD.org_id
    AND NEW.workspace_id IS OLD.workspace_id AND NEW.actor_id IS OLD.actor_id
    AND NEW.actor_kind IS OLD.actor_kind AND NEW.action IS OLD.action
    AND NEW.target_id IS OLD.target_id AND NEW.subject_id IS OLD.subject_id
    AND NEW.created_at IS OLD.created_at AND NEW.detail IS OLD.detail
    AND EXISTS (SELECT 1 FROM users
                WHERE id IN (OLD.actor_id, OLD.subject_id) AND erase_started_at > 0))
BEGIN
    SELECT RAISE(ABORT, 'a record of the audit log is not changed or removed');
END;

CREATE TRIGGER audit_log_kept BEFORE DELETE ON audit_log
WHEN EXISTS (SELECT 1 FROM organizations WHERE id = OLD.org_id)
    AND (OLD.workspace_id IS NULL
         OR EXISTS (SELECT 1 FROM workspaces WHERE id = OLD.workspace_id))
BEGIN
    SELECT RAISE(ABORT, 'a record of the audit log is not changed or removed');
END;
