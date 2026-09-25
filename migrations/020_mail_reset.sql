-- Password-reset tokens (source stores these in Redis). Hashed at rest,
-- single-use, 15-minute TTL. App role never SELECTs token_hash; issue/consume
-- go through SECURITY DEFINER functions.

CREATE TABLE fvoci.magic_tokens (
    token_hash text PRIMARY KEY,
    kind text NOT NULL,
    user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    generation integer NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT magic_tokens_kind_check CHECK (kind IN ('password_reset'))
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

-- Start the mail consumer after already-recorded events so an upgrade does
-- not send mail about past comments. --recover-outbox replays its window;
-- processed_events keeps a successful send from duplicating. SMTP that was
-- accepted before the cursor advanced is the documented at-least-once edge.
DO $$
BEGIN
    PERFORM pg_catalog.set_config('app.system_ctx', 'on', true);
    INSERT INTO fvoci.outbox_consumers (consumer, last_xact, last_seq)
    SELECT 'mail', e.xact, e.seq
    FROM fvoci.events AS e
    WHERE e.xact < pg_catalog.pg_snapshot_xmin(pg_catalog.pg_current_snapshot())
    ORDER BY e.xact DESC, e.seq DESC
    LIMIT 1
    ON CONFLICT (consumer) DO NOTHING;
    PERFORM pg_catalog.set_config('app.system_ctx', '', true);
END;
$$;
