-- Web Push: VAPID instance keys and user-global push subscriptions (source
-- contract SHA 393795261322b916e588043cf94feca999175843, push-subscriptions).

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
    -- Session that last registered this browser: its logout disconnects it.
    session_id uuid,
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

CREATE INDEX push_subscriptions_user_session_idx
    ON fvoci.push_subscriptions (user_id, session_id);

-- Pending sends fanned out by the `push` outbox consumer in the same
-- transaction as its processed mark. The sender claims rows, re-checks the
-- recipient right before each POST, and deletes them after the attempt.
CREATE TABLE fvoci.push_deliveries (
    id uuid PRIMARY KEY DEFAULT uuidv7() NOT NULL,
    event_id uuid NOT NULL,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    endpoint text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    claimed_until timestamptz,
    CONSTRAINT push_deliveries_event_user_endpoint_unique UNIQUE (event_id, user_id, endpoint)
);

ALTER TABLE fvoci.push_deliveries ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.push_deliveries FORCE ROW LEVEL SECURITY;
CREATE POLICY system_only ON fvoci.push_deliveries
    AS PERMISSIVE FOR ALL TO public
    USING ((SELECT public.app_system_ctx_on()))
    WITH CHECK ((SELECT public.app_system_ctx_on()));

-- No RLS: rows are user-global; the app reads/writes them in system context only.

CREATE OR REPLACE FUNCTION fvoci.app_vapid_public_key()
RETURNS text
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT c.vapid_public_key FROM fvoci.instance_config AS c WHERE c.id = 1
$$;

CREATE OR REPLACE FUNCTION fvoci.app_vapid_private_key()
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

CREATE OR REPLACE FUNCTION fvoci.app_set_vapid(p_public text, p_private text)
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
CREATE OR REPLACE FUNCTION fvoci.app_init_vapid(p_public text, p_private text)
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

-- Start the push cursor at the newest settled event (same as mail/webhooks):
-- an upgrade must not push every historical notification.
DO $$
BEGIN
    PERFORM pg_catalog.set_config('app.system_ctx', 'on', true);
    INSERT INTO fvoci.outbox_consumers (consumer, last_xact, last_seq)
    SELECT 'push', e.xact, e.seq
    FROM fvoci.events AS e
    WHERE e.xact < pg_catalog.pg_snapshot_xmin(pg_catalog.pg_current_snapshot())
    ORDER BY e.xact DESC, e.seq DESC
    LIMIT 1
    ON CONFLICT (consumer) DO NOTHING;
    PERFORM pg_catalog.set_config('app.system_ctx', '', true);
END;
$$;

REVOKE ALL ON FUNCTION fvoci.app_vapid_public_key() FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_vapid_private_key() FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_set_vapid(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_init_vapid(text, text) FROM PUBLIC;
