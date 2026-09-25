-- TOTP MFA, OIDC identity links, workspace SSO (source packages/db schema
-- identity.ts user_mfa/identity_links, ops.ts workspace_oidc). 027 and 028 are
-- reserved by open branches (integrations, collections); versions may gap.
--
-- The source keeps pending MFA challenges and OIDC flow state in Redis. Here
-- they are single-use rows: hashed keys, TTL checked at consume, deleted by
-- the consume (GETDEL) or by the maintenance GC. The app role has no table
-- privileges on them; issue/consume go through SECURITY DEFINER functions.

-- Source workspaces.auto_join_domains: JIT join for workspace SSO. The source
-- has no API for it either (set by the operator).
ALTER TABLE fvoci.workspaces
    ADD COLUMN auto_join_domains text[] NOT NULL DEFAULT '{}';

-- Secret is sealed (enc:v2, AAD `user-mfa:<user_id>`); recovery codes are
-- sha256 hex. last_used_step blocks replay of a TOTP step.
CREATE TABLE fvoci.user_mfa (
    user_id uuid PRIMARY KEY REFERENCES fvoci.users (id) ON DELETE CASCADE,
    totp_secret text NOT NULL,
    enabled_at timestamptz,
    recovery_hashes text[] NOT NULL DEFAULT '{}',
    last_used_step integer,
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

-- Tenant SSO stores provider_user_id as `<workspace_id>:<sub>`.
CREATE TABLE fvoci.identity_links (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES fvoci.users (id),
    provider text NOT NULL,
    provider_user_id text NOT NULL,
    email text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT identity_links_email_canonical_check CHECK (
        email IS NULL OR (email ~ '^[!-~]+$' AND email = lower(email COLLATE "C"))
    ),
    CONSTRAINT identity_links_provider_check CHECK (
        provider IN ('google', 'microsoft', 'kakao', 'naver', 'generic')
    ),
    CONSTRAINT identity_links_provider_provider_user_id_unique UNIQUE (provider, provider_user_id),
    CONSTRAINT identity_links_user_id_provider_unique UNIQUE (user_id, provider)
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

-- client_secret is sealed (AAD `workspace-oidc:<workspace_id>`).
CREATE TABLE fvoci.workspace_oidc (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    issuer text NOT NULL,
    client_id text NOT NULL,
    client_secret text NOT NULL,
    label text NOT NULL DEFAULT 'SSO',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT workspace_oidc_workspace_id_unique UNIQUE (workspace_id),
    CONSTRAINT workspace_oidc_secret_sealed_check CHECK (client_secret LIKE 'enc:v2:%')
);

ALTER TABLE fvoci.workspace_oidc ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.workspace_oidc FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.workspace_oidc
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    )
    WITH CHECK (
        workspace_id = (SELECT public.app_tenant_id())
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

-- Workspace SSO entry by slug before sign-in (source `workspaces.findBySlug`
-- in a system transaction): only the id of a live workspace that has a
-- configuration, nothing else.
CREATE FUNCTION fvoci.app_workspace_sso_id_by_slug(p_slug text)
RETURNS uuid
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT w.id
    FROM fvoci.workspaces AS w
    JOIN fvoci.workspace_oidc AS o ON o.workspace_id = w.id
    WHERE w.slug = p_slug AND w.deleted_at IS NULL
$$;

REVOKE ALL ON FUNCTION fvoci.app_mfa_challenge_issue(text, uuid, integer, timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_mfa_challenge_peek(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_mfa_challenge_consume(text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_oidc_state_issue(text, text, timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_oidc_state_consume(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_auth_ephemeral_purge_expired(timestamptz, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_workspace_sso_id_by_slug(text) FROM PUBLIC;
