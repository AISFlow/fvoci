CREATE SCHEMA IF NOT EXISTS fvoci;

CREATE TABLE fvoci.schema_migrations (
    version integer PRIMARY KEY,
    applied_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE fvoci.users (
    id uuid PRIMARY KEY,
    email text NOT NULL,
    password_hash text,
    given_name text NOT NULL,
    family_name text,
    text_scale smallint NOT NULL DEFAULT 16,
    locale text NOT NULL DEFAULT 'ko',
    timezone text NOT NULL DEFAULT 'Asia/Seoul',
    week_starts_on integer NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    email_verified_at timestamptz,
    anonymized_at timestamptz,
    suspended_at timestamptz,
    is_instance_admin boolean NOT NULL DEFAULT false,
    auth_generation integer NOT NULL DEFAULT 0,
    personal_workspace_id uuid,
    CONSTRAINT users_email_unique UNIQUE (email),
    CONSTRAINT users_email_canonical_check CHECK (
        email ~ '^[!-~]+$' AND email = lower(email COLLATE "C")
    ),
    CONSTRAINT users_week_starts_on_check CHECK (week_starts_on IN (0, 1)),
    CONSTRAINT users_text_scale_check CHECK (text_scale IN (16, 18, 20))
);

CREATE TABLE fvoci.workspaces (
    id uuid PRIMARY KEY,
    slug text NOT NULL,
    name text NOT NULL,
    settings jsonb NOT NULL DEFAULT '{}'::jsonb,
    kind text NOT NULL DEFAULT 'team',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    CONSTRAINT workspaces_slug_unique UNIQUE (slug),
    CONSTRAINT workspaces_slug_shape_check CHECK (slug ~ '^[a-z0-9-]{2,32}$'),
    CONSTRAINT workspaces_kind_check CHECK (kind IN ('team', 'personal'))
);

CREATE TABLE fvoci.memberships (
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id),
    user_id uuid NOT NULL REFERENCES fvoci.users (id),
    role text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, user_id),
    CONSTRAINT memberships_role_check CHECK (role IN ('owner', 'admin', 'member', 'guest'))
);

CREATE TABLE fvoci.sessions (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES fvoci.users (id),
    token_hash text NOT NULL,
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT sessions_token_hash_unique UNIQUE (token_hash)
);

CREATE SEQUENCE fvoci.events_seq;

CREATE TABLE fvoci.events (
    id uuid PRIMARY KEY,
    seq bigint NOT NULL DEFAULT nextval('fvoci.events_seq'),
    xact xid8 NOT NULL DEFAULT pg_current_xact_id(),
    workspace_id uuid,
    actor_user_id uuid,
    verb text NOT NULL,
    target_type text,
    target_id uuid,
    payload jsonb NOT NULL DEFAULT '{}',
    channel text NOT NULL DEFAULT 'web' CHECK (channel IN ('web', 'api', 'mcp', 'webhook', 'system')),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX events_relay_idx ON fvoci.events (xact, seq);
CREATE INDEX events_workspace_idx ON fvoci.events (workspace_id, created_at);

CREATE TABLE fvoci.audit_log (
    id uuid PRIMARY KEY,
    actor_user_id uuid,
    workspace_id uuid,
    verb text NOT NULL,
    target_type text,
    target_id uuid,
    payload jsonb NOT NULL DEFAULT '{}',
    ip inet,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX audit_log_created_at_id_idx ON fvoci.audit_log (created_at DESC, id DESC);
