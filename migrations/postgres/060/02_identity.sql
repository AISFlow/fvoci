-- FVOCI PostgreSQL baseline fvoci-postgres-060, step 02: identity.
-- users, workspaces, memberships, sessions, one-time tokens (password reset / login / email change), TOTP MFA,
-- OIDC identity links and flow state, legal documents and consents, and the narrow
-- SECURITY DEFINER functions through which the app role touches secret columns.
--
-- The app role keeps column-level grants on fvoci.users and fvoci.sessions
-- (scripts/grant-app-role.sql): it never reads password_hash, token_hash or
-- withdraw_cancel_token_hash and never writes email, deleted_at, anonymized_at,
-- email_verified_at or auth_generation directly.

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
    -- FK to fvoci.workspaces (users_personal_workspace_fk) is added below, after workspaces.
    personal_workspace_id uuid,
    withdraw_cancel_token_hash text,
    CONSTRAINT users_email_unique UNIQUE (email),
    CONSTRAINT users_email_canonical_check CHECK (
        email ~ '^[!-~]+$' AND email = lower(email COLLATE "C")
    ),
    CONSTRAINT users_week_starts_on_check CHECK (week_starts_on IN (0, 1)),
    CONSTRAINT users_text_scale_check CHECK (text_scale IN (16, 18, 20))
);

CREATE UNIQUE INDEX users_personal_workspace_id_unique
    ON fvoci.users (personal_workspace_id)
    WHERE personal_workspace_id IS NOT NULL;

CREATE UNIQUE INDEX users_withdraw_cancel_token_hash_unique
    ON fvoci.users (withdraw_cancel_token_hash)
    WHERE withdraw_cancel_token_hash IS NOT NULL;

CREATE INDEX users_withdrawn_due_idx
    ON fvoci.users (deleted_at)
    WHERE deleted_at IS NOT NULL AND anonymized_at IS NULL;

CREATE TABLE fvoci.workspaces (
    id uuid PRIMARY KEY,
    slug text NOT NULL,
    name text NOT NULL,
    settings jsonb NOT NULL DEFAULT '{}'::jsonb,
    kind text NOT NULL DEFAULT 'team',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    next_document_number integer NOT NULL DEFAULT 0,
    -- JIT join for workspace SSO (operator-set; no API in the source either).
    auto_join_domains text[] NOT NULL DEFAULT '{}',
    CONSTRAINT workspaces_slug_unique UNIQUE (slug),
    CONSTRAINT workspaces_slug_shape_check CHECK (slug ~ '^[a-z0-9-]{2,32}$'),
    CONSTRAINT workspaces_kind_check CHECK (kind IN ('team', 'personal'))
);

ALTER TABLE fvoci.users
    ADD CONSTRAINT users_personal_workspace_fk
    FOREIGN KEY (personal_workspace_id)
    REFERENCES fvoci.workspaces (id)
    ON DELETE SET NULL;

CREATE TABLE fvoci.memberships (
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id),
    user_id uuid NOT NULL REFERENCES fvoci.users (id),
    role text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, user_id),
    CONSTRAINT memberships_role_check CHECK (role IN ('owner', 'admin', 'member', 'guest'))
);

ALTER TABLE fvoci.workspaces ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.memberships ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON fvoci.workspaces
    AS PERMISSIVE FOR ALL TO public
    USING (
        id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

CREATE POLICY tenant_isolation ON fvoci.memberships
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()));

-- A user sees their own memberships across workspaces (workspace switcher).
CREATE POLICY memberships_select_self ON fvoci.memberships
    AS PERMISSIVE FOR SELECT TO public
    USING (user_id = (SELECT public.app_self_user_id()));

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

CREATE FUNCTION fvoci.app_user_password_hash(p_id uuid)
RETURNS text
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT password_hash FROM fvoci.users
    WHERE id = p_id AND deleted_at IS NULL
$$;

CREATE FUNCTION fvoci.app_session_by_token_hash(p_hash text)
RETURNS SETOF fvoci.sessions
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT * FROM fvoci.sessions
    WHERE token_hash = p_hash
      AND revoked_at IS NULL
      AND expires_at > now()
$$;

CREATE FUNCTION fvoci.app_user_rehash_password_hash(p_id uuid, p_hash text, p_expected text)
RETURNS integer
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    WITH updated AS (
        UPDATE fvoci.users
        SET password_hash = p_hash, updated_at = now()
        WHERE id = p_id AND deleted_at IS NULL AND password_hash = p_expected
        RETURNING 1
    )
    SELECT count(*)::integer FROM updated
$$;

-- One-time tokens (source keeps login / password_reset / email_change payloads in
-- one Redis namespace and GETDELs before checking the kind). Hashed at rest,
-- single-use, TTL checked at consume. The app role never SELECTs token_hash;
-- issue/consume go through the SECURITY DEFINER functions below.
CREATE TABLE fvoci.magic_tokens (
    token_hash text PRIMARY KEY,
    kind text NOT NULL,
    user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    generation integer NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    new_email text,
    CONSTRAINT magic_tokens_kind_check CHECK (kind IN ('password_reset', 'login', 'email_change')),
    CONSTRAINT magic_tokens_new_email_check CHECK (
        (kind = 'email_change') = (new_email IS NOT NULL)
        AND (new_email IS NULL OR (new_email ~ '^[!-~]+$' AND new_email = lower(new_email COLLATE "C")))
    )
);

CREATE INDEX magic_tokens_expires_at_idx ON fvoci.magic_tokens (expires_at);
CREATE INDEX magic_tokens_user_id_idx ON fvoci.magic_tokens (user_id);

ALTER TABLE fvoci.magic_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.magic_tokens FORCE ROW LEVEL SECURITY;
CREATE POLICY owner_isolation ON fvoci.magic_tokens
    AS PERMISSIVE FOR ALL TO public
    USING ((SELECT public.app_system_ctx_on()))
    WITH CHECK ((SELECT public.app_system_ctx_on()));

-- Credential replacement, not rehash: bump auth_generation in the same UPDATE
-- so leftover magic tokens and sessions fail the generation check. A withdrawn
-- row (deleted_at set, not anonymized) cannot change the hash.
CREATE FUNCTION fvoci.app_user_set_password_hash(p_id uuid, p_hash text)
RETURNS void
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    UPDATE fvoci.users
    SET password_hash = p_hash,
        auth_generation = auth_generation + 1,
        updated_at = now()
    WHERE id = p_id
      AND (
          deleted_at IS NULL
          OR (p_hash IS NULL AND anonymized_at IS NOT NULL)
      )
$$;

CREATE FUNCTION fvoci.app_magic_issue(
    p_hash text,
    p_kind text,
    p_user_id uuid,
    p_generation integer,
    p_expires_at timestamptz
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
BEGIN
    INSERT INTO fvoci.magic_tokens (token_hash, kind, user_id, generation, expires_at)
    VALUES (p_hash, p_kind, p_user_id, p_generation, p_expires_at);
END;
$$;

-- GETDEL: delete and return if unexpired. Expired rows are deleted and yield
-- no row, matching Redis GETDEL + TTL.
CREATE FUNCTION fvoci.app_magic_consume(p_hash text)
RETURNS TABLE (kind text, user_id uuid, generation integer)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
BEGIN
    RETURN QUERY
    DELETE FROM fvoci.magic_tokens
    WHERE token_hash = p_hash
      AND expires_at > now()
    RETURNING fvoci.magic_tokens.kind, fvoci.magic_tokens.user_id, fvoci.magic_tokens.generation;
END;
$$;

CREATE FUNCTION fvoci.app_magic_issue_email_change(
    p_hash text,
    p_user_id uuid,
    p_generation integer,
    p_new_email text,
    p_expires_at timestamptz
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
BEGIN
    INSERT INTO fvoci.magic_tokens (token_hash, kind, user_id, generation, expires_at, new_email)
    VALUES (p_hash, 'email_change', p_user_id, p_generation, p_expires_at, p_new_email);
END;
$$;

-- GETDEL with the email-change payload. Expired rows yield nothing.
CREATE FUNCTION fvoci.app_magic_consume_payload(p_hash text)
RETURNS TABLE (kind text, user_id uuid, generation integer, new_email text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
BEGIN
    RETURN QUERY
    DELETE FROM fvoci.magic_tokens
    WHERE token_hash = p_hash
      AND expires_at > now()
    RETURNING fvoci.magic_tokens.kind, fvoci.magic_tokens.user_id,
              fvoci.magic_tokens.generation, fvoci.magic_tokens.new_email;
END;
$$;

-- Bounded GC for a table the app role cannot touch directly (REVOKE ALL).
-- Returns counts only; never hashes or tokens.
CREATE FUNCTION fvoci.app_magic_purge_expired(p_now timestamptz, p_limit integer)
RETURNS integer
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    deleted integer;
    v_prev text;
BEGIN
    IF p_now IS NULL THEN
        RAISE EXCEPTION 'app_magic_purge_expired: now is required';
    END IF;
    IF p_limit IS NULL OR p_limit < 1 THEN
        RAISE EXCEPTION 'app_magic_purge_expired: limit must be a positive integer';
    END IF;

    v_prev := COALESCE(pg_catalog.current_setting('app.system_ctx', true), '');
    PERFORM pg_catalog.set_config('app.system_ctx', 'on', true);

    WITH doomed AS (
        SELECT token_hash
        FROM fvoci.magic_tokens
        WHERE expires_at <= p_now
        ORDER BY expires_at ASC, token_hash ASC
        LIMIT p_limit
        FOR UPDATE SKIP LOCKED
    )
    DELETE FROM fvoci.magic_tokens AS t
    USING doomed
    WHERE t.token_hash = doomed.token_hash;

    GET DIAGNOSTICS deleted = ROW_COUNT;
    PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
    RETURN deleted;
END;
$$;

-- Account lifecycle: withdraw grace period, withdraw cancel token, final
-- anonymization, email change and verification.

-- Source markWithdrawn + setWithdrawCancelTokenHash in one statement.
-- auth_generation bumps so outstanding magic tokens fail their check.
CREATE FUNCTION fvoci.app_user_withdraw(p_id uuid, p_at timestamptz, p_cancel_hash text)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    updated integer;
BEGIN
    IF p_at IS NULL OR p_cancel_hash IS NULL OR p_cancel_hash !~ '^[0-9a-f]{64}$' THEN
        RAISE EXCEPTION 'app_user_withdraw: invalid arguments';
    END IF;
    UPDATE fvoci.users
    SET deleted_at = p_at,
        withdraw_cancel_token_hash = p_cancel_hash,
        auth_generation = auth_generation + 1,
        updated_at = now()
    WHERE id = p_id
      AND deleted_at IS NULL
      AND anonymized_at IS NULL;
    GET DIAGNOSTICS updated = ROW_COUNT;
    RETURN updated = 1;
END;
$$;

CREATE FUNCTION fvoci.app_user_id_by_withdraw_cancel_token_hash(p_hash text)
RETURNS uuid
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT id FROM fvoci.users
    WHERE withdraw_cancel_token_hash = p_hash
      AND deleted_at IS NOT NULL
      AND anonymized_at IS NULL
$$;

-- Source restoreWithdrawn: clear deleted_at and the one-time cancel token.
-- The definer itself requires the matching cancel hash and an unexpired grace
-- period (source deadline: now < deleted_at + 14 days).
CREATE FUNCTION fvoci.app_user_restore_withdrawn(p_id uuid, p_cancel_hash text)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    updated integer;
BEGIN
    IF p_cancel_hash IS NULL OR p_cancel_hash !~ '^[0-9a-f]{64}$' THEN
        RETURN false;
    END IF;
    UPDATE fvoci.users
    SET deleted_at = NULL,
        withdraw_cancel_token_hash = NULL,
        updated_at = now()
    WHERE id = p_id
      AND withdraw_cancel_token_hash = p_cancel_hash
      AND deleted_at IS NOT NULL
      AND deleted_at > now() - interval '14 days'
      AND anonymized_at IS NULL;
    GET DIAGNOSTICS updated = ROW_COUNT;
    RETURN updated = 1;
END;
$$;

-- Source anonymize: only a row whose 14-day grace period has ended. The
-- cutoff is capped inside the definer: a later p_due_before can never erase
-- early. Clears name, email, password hash and the cancel token in one UPDATE.
CREATE FUNCTION fvoci.app_user_anonymize(
    p_id uuid,
    p_given_name text,
    p_email text,
    p_at timestamptz,
    p_due_before timestamptz
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    updated integer;
BEGIN
    IF p_email IS NULL OR p_email !~ '^withdrawn-[0-9a-f]{12}@withdrawn\.invalid$' THEN
        RAISE EXCEPTION 'app_user_anonymize: invalid placeholder email';
    END IF;
    IF p_at IS NULL OR p_due_before IS NULL THEN
        RAISE EXCEPTION 'app_user_anonymize: time arguments are required';
    END IF;
    UPDATE fvoci.users
    SET given_name = p_given_name,
        family_name = NULL,
        email = p_email,
        password_hash = NULL,
        withdraw_cancel_token_hash = NULL,
        anonymized_at = p_at,
        auth_generation = auth_generation + 1,
        updated_at = now()
    WHERE id = p_id
      AND deleted_at IS NOT NULL
      AND deleted_at <= LEAST(p_due_before, now() - interval '14 days')
      AND anonymized_at IS NULL;
    GET DIAGNOSTICS updated = ROW_COUNT;
    RETURN updated = 1;
END;
$$;

-- Source updateEmail: live row only; a unique violation is reported, not raised.
-- An email change cuts off the previous mailbox: auth_generation bumps in the
-- same UPDATE so mailed links and pending MFA challenges fail their check.
CREATE FUNCTION fvoci.app_user_update_email(p_id uuid, p_email text)
RETURNS text
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    updated integer;
BEGIN
    -- Same canonical form as users/magic_tokens: printable ASCII, lower case,
    -- one local part and one domain.
    IF p_email IS NULL
       OR p_email !~ '^[!-~]+$'
       OR p_email <> lower(p_email COLLATE "C")
       OR p_email !~ '^[^@]+@[^@]+$'
       OR length(p_email) > 320 THEN
        RAISE EXCEPTION 'app_user_update_email: invalid email';
    END IF;
    BEGIN
        UPDATE fvoci.users
        SET email = p_email,
            email_verified_at = now(),
            auth_generation = auth_generation + 1,
            updated_at = now()
        WHERE id = p_id
          AND deleted_at IS NULL;
        GET DIAGNOSTICS updated = ROW_COUNT;
    EXCEPTION WHEN unique_violation THEN
        RETURN 'email_taken';
    END;
    IF updated = 1 THEN
        RETURN 'ok';
    END IF;
    RETURN 'not_found';
END;
$$;

-- Source setEmailVerified for a consumed login link.
CREATE FUNCTION fvoci.app_user_mark_email_verified(p_id uuid)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    updated integer;
BEGIN
    UPDATE fvoci.users
    SET email_verified_at = now(), updated_at = now()
    WHERE id = p_id AND deleted_at IS NULL;
    GET DIAGNOSTICS updated = ROW_COUNT;
    RETURN updated = 1;
END;
$$;

-- Instance administration: legal documents and consents, and the definers the
-- admin console and the 428 consent gate need.
CREATE TABLE fvoci.legal_documents (
    id uuid PRIMARY KEY,
    kind text NOT NULL,
    version integer NOT NULL,
    title text NOT NULL,
    body_markdown text NOT NULL,
    -- Rendered once at publish (markdown -> editor schema -> sanitized HTML);
    -- rows are immutable, so reads never render.
    body_html text NOT NULL,
    required boolean NOT NULL,
    effective_at timestamptz NOT NULL,
    published_at timestamptz NOT NULL,
    created_by uuid NOT NULL REFERENCES fvoci.users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT legal_documents_kind_version_unique UNIQUE (kind, version),
    CONSTRAINT legal_documents_kind_shape_check CHECK (kind ~ '^[a-z0-9-]{1,50}$'),
    CONSTRAINT legal_documents_version_check CHECK (version >= 1)
);

-- Rows are append-only: the app role gets SELECT and INSERT only. Anyone may read
-- published documents (GET /legal/:kind is public); only an instance-admin
-- transaction that has switched to the system context may publish.
ALTER TABLE fvoci.legal_documents ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.legal_documents FORCE ROW LEVEL SECURITY;

CREATE POLICY legal_documents_select ON fvoci.legal_documents
    FOR SELECT USING (true);

CREATE POLICY legal_documents_insert ON fvoci.legal_documents
    FOR INSERT WITH CHECK ((SELECT public.app_system_ctx_on()));

CREATE TABLE fvoci.user_consents (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES fvoci.users (id),
    kind text NOT NULL,
    version integer NOT NULL,
    consented_at timestamptz NOT NULL,
    ip inet,
    channel text NOT NULL,
    CONSTRAINT user_consents_user_id_kind_version_unique UNIQUE (user_id, kind, version),
    CONSTRAINT user_consents_channel_check CHECK (channel IN ('signup', 'gate'))
);

-- A user reads and records their own consents; a workspace admin reads the
-- consents of the tenant's members. Consents are evidence: append-only.
ALTER TABLE fvoci.user_consents ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.user_consents FORCE ROW LEVEL SECURITY;

CREATE POLICY user_consents_select ON fvoci.user_consents
    FOR SELECT USING (
        user_id = (SELECT public.app_self_user_id())
        OR (SELECT public.app_system_ctx_on())
        OR EXISTS (
            SELECT 1 FROM fvoci.memberships m
            WHERE m.user_id = user_consents.user_id
              AND m.workspace_id = (SELECT public.app_tenant_id())
        )
    );

CREATE POLICY user_consents_insert ON fvoci.user_consents
    FOR INSERT WITH CHECK (
        user_id = (SELECT public.app_self_user_id())
        OR (SELECT public.app_system_ctx_on())
    );

-- The 428 consent gate: does the live session behind this token hash belong
-- to a live user who has not consented to the latest version of every kind
-- whose latest version is required? Returns only that boolean.
CREATE FUNCTION fvoci.app_session_consent_pending(p_hash text)
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM fvoci.sessions s
        INNER JOIN fvoci.users u ON u.id = s.user_id
        INNER JOIN (
            SELECT DISTINCT ON (d.kind) d.kind, d.version, d.required
            FROM fvoci.legal_documents d
            ORDER BY d.kind, d.version DESC
        ) latest ON latest.required
        WHERE s.token_hash = p_hash
          AND s.revoked_at IS NULL
          AND s.expires_at > now()
          AND u.deleted_at IS NULL
          AND u.suspended_at IS NULL
          AND NOT EXISTS (
              SELECT 1 FROM fvoci.user_consents c
              WHERE c.user_id = u.id
                AND c.kind = latest.kind
                AND c.version = latest.version
          )
    )
$$;

-- users.is_instance_admin / suspended_at are not app-role writable columns.
-- These definers change one flag of one live user, only when the calling
-- transaction's self user (app.self_user_id) is a live instance admin, and
-- refuse any change that would leave the instance without a live admin.
CREATE FUNCTION fvoci.app_admin_set_instance_admin(p_target uuid, p_value boolean)
RETURNS integer
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    changed integer;
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM fvoci.users a
        WHERE a.id = public.app_self_user_id()
          AND a.is_instance_admin
          AND a.deleted_at IS NULL
          AND a.suspended_at IS NULL
    ) THEN
        RAISE EXCEPTION 'instance admin required' USING ERRCODE = '42501';
    END IF;
    UPDATE fvoci.users
    SET is_instance_admin = p_value, updated_at = now()
    WHERE id = p_target AND deleted_at IS NULL AND is_instance_admin IS DISTINCT FROM p_value;
    GET DIAGNOSTICS changed = ROW_COUNT;
    IF NOT EXISTS (
        SELECT 1 FROM fvoci.users
        WHERE is_instance_admin AND deleted_at IS NULL AND suspended_at IS NULL
    ) THEN
        RAISE EXCEPTION 'last instance admin' USING ERRCODE = '23514';
    END IF;
    RETURN changed;
END
$$;

CREATE FUNCTION fvoci.app_admin_set_suspended(p_target uuid, p_suspended boolean)
RETURNS integer
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    changed integer;
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM fvoci.users a
        WHERE a.id = public.app_self_user_id()
          AND a.is_instance_admin
          AND a.deleted_at IS NULL
          AND a.suspended_at IS NULL
    ) THEN
        RAISE EXCEPTION 'instance admin required' USING ERRCODE = '42501';
    END IF;
    IF p_suspended THEN
        UPDATE fvoci.users
        SET suspended_at = now(), updated_at = now()
        WHERE id = p_target AND deleted_at IS NULL AND suspended_at IS NULL;
    ELSE
        UPDATE fvoci.users
        SET suspended_at = NULL, updated_at = now()
        WHERE id = p_target AND deleted_at IS NULL AND suspended_at IS NOT NULL;
    END IF;
    GET DIAGNOSTICS changed = ROW_COUNT;
    IF NOT EXISTS (
        SELECT 1 FROM fvoci.users
        WHERE is_instance_admin AND deleted_at IS NULL AND suspended_at IS NULL
    ) THEN
        RAISE EXCEPTION 'last instance admin' USING ERRCODE = '23514';
    END IF;
    RETURN changed;
END
$$;

-- Admin user erasure cancel. The user's own cancel path
-- (app_user_restore_withdrawn) requires the one-time cancel hash, which an
-- instance admin never sees. The caller must have set app.self_user_id to a live
-- instance admin; the 14-day grace period is checked against the wall clock.
CREATE FUNCTION fvoci.app_admin_user_restore_withdrawn(p_id uuid)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    updated integer;
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM fvoci.users a
        WHERE a.id = public.app_self_user_id()
          AND a.is_instance_admin
          AND a.deleted_at IS NULL
          AND a.suspended_at IS NULL
    ) THEN
        RAISE EXCEPTION 'instance admin required' USING ERRCODE = '42501';
    END IF;
    UPDATE fvoci.users
    SET deleted_at = NULL,
        withdraw_cancel_token_hash = NULL,
        updated_at = now()
    WHERE id = p_id
      AND deleted_at IS NOT NULL
      AND deleted_at > clock_timestamp() - interval '14 days'
      AND anonymized_at IS NULL;
    GET DIAGNOSTICS updated = ROW_COUNT;
    RETURN updated = 1;
END;
$$;

-- TOTP MFA, OIDC identity links and flow state. Pending MFA challenges and OIDC
-- flow state are single-use rows: hashed keys, TTL checked at consume, deleted by
-- the consume (GETDEL) or by the maintenance GC. The app role has no table
-- privileges on them; issue/consume go through SECURITY DEFINER functions.

-- Secret is sealed (enc:v2, AAD `user-mfa:<user_id>`); recovery codes are
-- sha256 hex. last_used_step blocks replay of a TOTP step.
CREATE TABLE fvoci.user_mfa (
    user_id uuid PRIMARY KEY REFERENCES fvoci.users (id) ON DELETE CASCADE,
    totp_secret text NOT NULL,
    enabled_at timestamptz,
    recovery_hashes text[] NOT NULL DEFAULT '{}',
    last_used_step integer,
    -- Per-account verify attempts in the current window; kept in the database
    -- so the cap holds across restarts and replicas.
    verify_window_start timestamptz,
    verify_count integer NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT user_mfa_secret_sealed_check CHECK (totp_secret LIKE 'enc:v2:%')
);

ALTER TABLE fvoci.user_mfa ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.user_mfa FORCE ROW LEVEL SECURITY;
CREATE POLICY owner_isolation ON fvoci.user_mfa
    AS PERMISSIVE FOR ALL TO public
    USING (
        user_id = (SELECT public.app_self_user_id())
        OR (SELECT public.app_system_ctx_on())
    )
    WITH CHECK (
        user_id = (SELECT public.app_self_user_id())
        OR (SELECT public.app_system_ctx_on())
    );

-- Tenant SSO stores provider_user_id as `<workspace_id>:<sub>`. A provider
-- subject is only unique within its issuer: issuer records the issuer the link
-- was verified against (NULL only for links that predate issuer provenance;
-- such a link is re-created by authenticated unlink and reconnect, never backfilled).
CREATE TABLE fvoci.identity_links (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES fvoci.users (id),
    provider text NOT NULL,
    provider_user_id text NOT NULL,
    email text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    issuer text,
    CONSTRAINT identity_links_email_canonical_check CHECK (
        email IS NULL OR (email ~ '^[!-~]+$' AND email = lower(email COLLATE "C"))
    ),
    CONSTRAINT identity_links_provider_check CHECK (
        provider IN ('google', 'microsoft', 'kakao', 'naver', 'generic')
    ),
    CONSTRAINT identity_links_provider_provider_user_id_unique UNIQUE (provider, provider_user_id),
    CONSTRAINT identity_links_user_id_provider_unique UNIQUE (user_id, provider),
    CONSTRAINT identity_links_issuer_nonempty_check CHECK (issuer IS NULL OR issuer <> '')
);

-- Sign-in looks a link up before any user is known: system context.
ALTER TABLE fvoci.identity_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.identity_links FORCE ROW LEVEL SECURITY;
CREATE POLICY owner_isolation ON fvoci.identity_links
    AS PERMISSIVE FOR ALL TO public
    USING (
        user_id = (SELECT public.app_self_user_id())
        OR (SELECT public.app_system_ctx_on())
    )
    WITH CHECK (
        user_id = (SELECT public.app_self_user_id())
        OR (SELECT public.app_system_ctx_on())
    );

-- Pending second factor (source Redis `mfa:pending:<hash>`, 5 minutes).
CREATE TABLE fvoci.mfa_challenges (
    token_hash text PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    generation integer NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX mfa_challenges_expires_at_idx ON fvoci.mfa_challenges (expires_at);
CREATE INDEX mfa_challenges_user_id_idx ON fvoci.mfa_challenges (user_id);
ALTER TABLE fvoci.mfa_challenges ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.mfa_challenges FORCE ROW LEVEL SECURITY;
CREATE POLICY system_only ON fvoci.mfa_challenges
    AS PERMISSIVE FOR ALL TO public
    USING ((SELECT public.app_system_ctx_on()))
    WITH CHECK ((SELECT public.app_system_ctx_on()));

-- OIDC flow state (source Redis `oidc:state:v2:<hash>`, 10 minutes). The
-- payload holds the nonce and PKCE verifier and is sealed like the source.
CREATE TABLE fvoci.oidc_states (
    state_hash text PRIMARY KEY,
    payload text NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT oidc_states_payload_sealed_check CHECK (payload LIKE 'enc:v2:%')
);
CREATE INDEX oidc_states_expires_at_idx ON fvoci.oidc_states (expires_at);
ALTER TABLE fvoci.oidc_states ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.oidc_states FORCE ROW LEVEL SECURITY;
CREATE POLICY system_only ON fvoci.oidc_states
    AS PERMISSIVE FOR ALL TO public
    USING ((SELECT public.app_system_ctx_on()))
    WITH CHECK ((SELECT public.app_system_ctx_on()));

CREATE FUNCTION fvoci.app_mfa_challenge_issue(
    p_hash text,
    p_user_id uuid,
    p_generation integer,
    p_expires_at timestamptz
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
BEGIN
    INSERT INTO fvoci.mfa_challenges (token_hash, user_id, generation, expires_at)
    VALUES (p_hash, p_user_id, p_generation, p_expires_at);
END;
$$;

-- Read without consuming: a wrong code keeps the challenge (source deletes
-- only on success; the per-account limit bounds guessing).
CREATE FUNCTION fvoci.app_mfa_challenge_peek(p_hash text)
RETURNS TABLE (user_id uuid, generation integer)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT c.user_id, c.generation
    FROM fvoci.mfa_challenges AS c
    WHERE c.token_hash = p_hash AND c.expires_at > now()
$$;

-- Consume on success. Returns false when another request already used it.
CREATE FUNCTION fvoci.app_mfa_challenge_consume(p_hash text, p_user_id uuid)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    deleted integer;
BEGIN
    DELETE FROM fvoci.mfa_challenges
    WHERE token_hash = p_hash AND user_id = p_user_id AND expires_at > now();
    GET DIAGNOSTICS deleted = ROW_COUNT;
    RETURN deleted = 1;
END;
$$;

-- Counts one verify attempt for the account. Returns 0 while the attempt is
-- within p_limit per p_window_seconds, otherwise the seconds until the window
-- ends (the Retry-After). Runs under the system context because the caller has
-- no session yet (user_mfa is owner-or-system under FORCE RLS). An account
-- without an MFA row counts nothing and gets 0; verification then fails.
CREATE FUNCTION fvoci.app_mfa_verify_attempt(p_user_id uuid, p_limit integer, p_window_seconds integer)
RETURNS integer
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    v_prev text := current_setting('app.system_ctx', true);
    v_count integer;
    v_start timestamptz;
BEGIN
    IF p_limit < 1 OR p_window_seconds < 1 OR p_window_seconds > 86400 THEN
        RAISE EXCEPTION 'invalid verify limit' USING ERRCODE = '22023';
    END IF;
    PERFORM pg_catalog.set_config('app.system_ctx', 'on', true);
    UPDATE fvoci.user_mfa
    SET verify_count = CASE
            WHEN verify_window_start > now() - make_interval(secs => p_window_seconds)
            THEN verify_count + 1 ELSE 1 END,
        verify_window_start = CASE
            WHEN verify_window_start > now() - make_interval(secs => p_window_seconds)
            THEN verify_window_start ELSE now() END
    WHERE user_id = p_user_id
    RETURNING verify_count, verify_window_start INTO v_count, v_start;
    PERFORM pg_catalog.set_config('app.system_ctx', COALESCE(v_prev, ''), true);
    IF v_count IS NULL OR v_count <= p_limit THEN
        RETURN 0;
    END IF;
    RETURN GREATEST(1, ceil(extract(epoch FROM
        (v_start + make_interval(secs => p_window_seconds)) - now()))::integer);
END;
$$;

CREATE FUNCTION fvoci.app_oidc_state_issue(p_hash text, p_payload text, p_expires_at timestamptz)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
BEGIN
    INSERT INTO fvoci.oidc_states (state_hash, payload, expires_at)
    VALUES (p_hash, p_payload, p_expires_at);
END;
$$;

-- GETDEL: single use; an expired row is deleted and yields nothing.
CREATE FUNCTION fvoci.app_oidc_state_consume(p_hash text)
RETURNS text
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    v_payload text;
    v_expires timestamptz;
BEGIN
    DELETE FROM fvoci.oidc_states
    WHERE state_hash = p_hash
    RETURNING payload, expires_at INTO v_payload, v_expires;
    IF v_payload IS NULL OR v_expires <= now() THEN
        RETURN NULL;
    END IF;
    RETURN v_payload;
END;
$$;

CREATE FUNCTION fvoci.app_auth_ephemeral_purge_expired(p_now timestamptz, p_limit integer)
RETURNS integer
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    challenges integer;
    states integer;
BEGIN
    IF p_now IS NULL THEN
        RAISE EXCEPTION 'app_auth_ephemeral_purge_expired: now is required';
    END IF;
    IF p_limit IS NULL OR p_limit < 1 THEN
        RAISE EXCEPTION 'app_auth_ephemeral_purge_expired: limit must be a positive integer';
    END IF;
    WITH doomed AS (
        SELECT token_hash FROM fvoci.mfa_challenges
        WHERE expires_at <= p_now
        ORDER BY expires_at, token_hash
        LIMIT p_limit
        FOR UPDATE SKIP LOCKED
    )
    DELETE FROM fvoci.mfa_challenges AS t USING doomed
    WHERE t.token_hash = doomed.token_hash;
    GET DIAGNOSTICS challenges = ROW_COUNT;
    WITH doomed AS (
        SELECT state_hash FROM fvoci.oidc_states
        WHERE expires_at <= p_now
        ORDER BY expires_at, state_hash
        LIMIT p_limit
        FOR UPDATE SKIP LOCKED
    )
    DELETE FROM fvoci.oidc_states AS t USING doomed
    WHERE t.state_hash = doomed.state_hash;
    GET DIAGNOSTICS states = ROW_COUNT;
    RETURN challenges + states;
END;
$$;

-- PUBLIC execute is revoked by the runner for every definer the migration owner
-- created; these explicit revokes keep the contract visible in the source.
REVOKE ALL ON FUNCTION fvoci.app_magic_purge_expired(timestamptz, integer) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION fvoci.app_admin_user_restore_withdrawn(uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_mfa_challenge_issue(text, uuid, integer, timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_mfa_challenge_peek(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_mfa_challenge_consume(text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_mfa_verify_attempt(uuid, integer, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_oidc_state_issue(text, text, timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_oidc_state_consume(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_auth_ephemeral_purge_expired(timestamptz, integer) FROM PUBLIC;
