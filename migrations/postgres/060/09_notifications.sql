-- FVOCI PostgreSQL baseline fvoci-postgres-060, step 09: notifications and push.
-- In-app notifications and preferences, calendar feed tokens, the VAPID instance
-- key row and user-global Web Push subscriptions/deliveries.

CREATE TABLE fvoci.notifications (
    id uuid PRIMARY KEY DEFAULT uuidv7() NOT NULL,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    user_id uuid NOT NULL,
    event_id uuid NOT NULL,
    verb text NOT NULL,
    actor_user_id uuid,
    target_type text,
    target_id uuid,
    payload jsonb NOT NULL DEFAULT '{}'::jsonb,
    read_at timestamptz,
    archived_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT notifications_workspace_id_id_unique UNIQUE (workspace_id, id),
    -- One notification per recipient per event: outbox replay (e.g. after
    -- fvoci-migrate --recover-outbox on restore) must not duplicate.
    CONSTRAINT notifications_event_recipient_unique UNIQUE (workspace_id, user_id, event_id)
);

CREATE INDEX notifications_inbox_idx
    ON fvoci.notifications (workspace_id, user_id, created_at, id);

CREATE INDEX notifications_unread_idx
    ON fvoci.notifications (workspace_id, user_id)
    WHERE read_at IS NULL AND archived_at IS NULL;

ALTER TABLE fvoci.notifications ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.notifications FORCE ROW LEVEL SECURITY;
CREATE POLICY owner_isolation ON fvoci.notifications
    AS PERMISSIVE FOR ALL TO public
    USING (
        (
            workspace_id = (SELECT public.app_tenant_id())
            AND user_id = (SELECT public.app_self_user_id())
        )
        OR (SELECT public.app_system_ctx_on())
    )
    WITH CHECK (
        (
            workspace_id = (SELECT public.app_tenant_id())
            AND user_id = (SELECT public.app_self_user_id())
        )
        OR (SELECT public.app_system_ctx_on())
    );

CREATE TABLE fvoci.notification_prefs (
    workspace_id uuid NOT NULL,
    user_id uuid NOT NULL,
    in_app boolean NOT NULL DEFAULT true,
    mail_immediate boolean NOT NULL DEFAULT true,
    mail_digest boolean NOT NULL DEFAULT false,
    last_digest_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, user_id),
    CONSTRAINT notification_prefs_workspace_user_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id)
        ON DELETE CASCADE
);

CREATE INDEX notification_prefs_digest_due_idx
    ON fvoci.notification_prefs (last_digest_at)
    WHERE mail_digest = true;

ALTER TABLE fvoci.notification_prefs ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.notification_prefs FORCE ROW LEVEL SECURITY;
CREATE POLICY owner_isolation ON fvoci.notification_prefs
    AS PERMISSIVE FOR ALL TO public
    USING (
        (
            workspace_id = (SELECT public.app_tenant_id())
            AND user_id = (SELECT public.app_self_user_id())
        )
        OR (SELECT public.app_system_ctx_on())
    )
    WITH CHECK (
        (
            workspace_id = (SELECT public.app_tenant_id())
            AND user_id = (SELECT public.app_self_user_id())
        )
        OR (SELECT public.app_system_ctx_on())
    );

CREATE TABLE fvoci.ics_tokens (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    user_id uuid NOT NULL,
    token_hash text NOT NULL,
    expires_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT ics_tokens_token_hash_unique UNIQUE (token_hash),
    CONSTRAINT ics_tokens_workspace_id_user_id_unique UNIQUE (workspace_id, user_id),
    CONSTRAINT ics_tokens_workspace_user_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id)
        ON DELETE CASCADE
);

CREATE INDEX ics_tokens_expires_at_idx ON fvoci.ics_tokens (expires_at);

ALTER TABLE fvoci.ics_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.ics_tokens FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.ics_tokens
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    )
    WITH CHECK (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

-- Web Push: VAPID instance keys (singleton row seeded empty; the first server
-- start stores the pair) and user-global push subscriptions. No RLS on
-- instance_config and push_subscriptions: rows are user-global; the app
-- reads/writes them in system context only, and the app role sees only
-- vapid_public_key (grant-app-role.sql).
CREATE TABLE fvoci.instance_config (
    id integer PRIMARY KEY CHECK (id = 1),
    vapid_public_key text,
    vapid_private_key text,
    CONSTRAINT instance_config_private_sealed_check CHECK (
        vapid_private_key IS NULL OR vapid_private_key LIKE 'enc:v2:%'
    )
);

INSERT INTO fvoci.instance_config (id) VALUES (1);

CREATE TABLE fvoci.push_subscriptions (
    id uuid PRIMARY KEY DEFAULT uuidv7() NOT NULL,
    user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    endpoint text NOT NULL,
    p256dh text NOT NULL,
    auth text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    -- Session that last registered this browser. Its logout disconnects the
    -- browser, and sends require it to be live (expired, revoked, reset and
    -- revoke-all sessions no longer authorize delivery).
    session_id uuid NOT NULL REFERENCES fvoci.sessions (id) ON DELETE CASCADE,
    CONSTRAINT push_subscriptions_user_endpoint_unique UNIQUE (user_id, endpoint),
    CONSTRAINT push_subscriptions_endpoint_len CHECK (octet_length(endpoint) <= 2048),
    -- Unpadded base64url length locks (65-byte P-256 public, 16-byte auth secret).
    CONSTRAINT push_subscriptions_p256dh_len CHECK (char_length(p256dh) = 87),
    CONSTRAINT push_subscriptions_auth_len CHECK (char_length(auth) = 22)
);

CREATE INDEX push_subscriptions_user_updated_idx
    ON fvoci.push_subscriptions (user_id, updated_at DESC);

CREATE INDEX push_subscriptions_endpoint_idx
    ON fvoci.push_subscriptions (endpoint);

CREATE INDEX push_subscriptions_session_idx
    ON fvoci.push_subscriptions (session_id);

-- Pending sends fanned out by the `push` outbox consumer in the same
-- transaction as its processed mark. Holds no endpoint, key or content: the
-- sender reads the subscription and rebuilds the payload when it re-checks
-- the recipient. `attempt` fences a claim; `handed_off_at` marks rows that
-- passed that check and may be sending.
CREATE TABLE fvoci.push_deliveries (
    id uuid PRIMARY KEY DEFAULT uuidv7() NOT NULL,
    event_id uuid NOT NULL,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    subscription_id uuid NOT NULL REFERENCES fvoci.push_subscriptions (id) ON DELETE CASCADE,
    attempt integer NOT NULL DEFAULT 0,
    claimed_until timestamptz,
    handed_off_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT push_deliveries_event_subscription_unique UNIQUE (event_id, subscription_id)
);

CREATE INDEX push_deliveries_subscription_idx ON fvoci.push_deliveries (subscription_id);

ALTER TABLE fvoci.push_deliveries ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.push_deliveries FORCE ROW LEVEL SECURITY;
CREATE POLICY system_only ON fvoci.push_deliveries
    AS PERMISSIVE FOR ALL TO public
    USING ((SELECT public.app_system_ctx_on()))
    WITH CHECK ((SELECT public.app_system_ctx_on()));

CREATE FUNCTION fvoci.app_vapid_public_key()
RETURNS text
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT c.vapid_public_key FROM fvoci.instance_config AS c WHERE c.id = 1
$$;

CREATE FUNCTION fvoci.app_vapid_private_key()
RETURNS text
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    sealed text;
BEGIN
    IF (SELECT public.app_system_ctx_on()) IS NOT TRUE THEN
        RAISE EXCEPTION 'vapid private key requires system context';
    END IF;
    SELECT c.vapid_private_key INTO sealed FROM fvoci.instance_config AS c WHERE c.id = 1;
    RETURN sealed;
END;
$$;

CREATE FUNCTION fvoci.app_set_vapid(p_public text, p_private text)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
BEGIN
    IF (SELECT public.app_system_ctx_on()) IS NOT TRUE THEN
        RAISE EXCEPTION 'set vapid requires system context';
    END IF;
    UPDATE fvoci.instance_config
    SET
        vapid_public_key = p_public,
        vapid_private_key = p_private
    WHERE id = 1;
END;
$$;

-- First writer wins: two replicas booting together must not overwrite each
-- other's keypair, or subscriptions made against the first public key would
-- be rejected forever. Returns whether this call stored the pair.
CREATE FUNCTION fvoci.app_init_vapid(p_public text, p_private text)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    stored boolean;
BEGIN
    IF (SELECT public.app_system_ctx_on()) IS NOT TRUE THEN
        RAISE EXCEPTION 'init vapid requires system context';
    END IF;
    UPDATE fvoci.instance_config
    SET
        vapid_public_key = p_public,
        vapid_private_key = p_private
    WHERE id = 1 AND vapid_private_key IS NULL
    RETURNING true INTO stored;
    RETURN coalesce(stored, false);
END;
$$;

REVOKE ALL ON FUNCTION fvoci.app_vapid_public_key() FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_vapid_private_key() FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_set_vapid(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_init_vapid(text, text) FROM PUBLIC;
