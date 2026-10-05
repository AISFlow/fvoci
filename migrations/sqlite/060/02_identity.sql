-- FVOCI SQLite-family new-install baseline, lineage fvoci-sqlite-060, step 02: identity.
-- users, workspaces, memberships, sessions, one-time tokens, MFA, identity links, OIDC flow state, legal documents and consents.

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
CREATE TABLE mfa_challenges (
    token_hash TEXT PRIMARY KEY NOT NULL,
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    generation INTEGER NOT NULL CHECK (generation BETWEEN -2147483648 AND 2147483647),
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE TABLE oidc_states (
    state_hash TEXT PRIMARY KEY NOT NULL,
    payload TEXT NOT NULL CHECK (substr(payload,1,7)='enc:v2:'),
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE UNIQUE INDEX users_personal_workspace_id_unique ON users(personal_workspace_id) WHERE personal_workspace_id IS NOT NULL;
CREATE UNIQUE INDEX users_withdraw_cancel_token_hash_unique ON users(withdraw_cancel_token_hash) WHERE withdraw_cancel_token_hash IS NOT NULL;
CREATE INDEX users_withdrawn_due_idx ON users(deleted_at) WHERE deleted_at IS NOT NULL AND anonymized_at IS NULL;
CREATE INDEX magic_tokens_expires_at_idx ON magic_tokens(expires_at);
CREATE INDEX magic_tokens_user_id_idx ON magic_tokens(user_id);
CREATE INDEX mfa_challenges_expires_at_idx ON mfa_challenges(expires_at);
CREATE INDEX mfa_challenges_user_id_idx ON mfa_challenges(user_id);
CREATE INDEX oidc_states_expires_at_idx ON oidc_states(expires_at);
