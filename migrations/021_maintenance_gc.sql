-- Bounded GC for tables the app role cannot touch directly (REVOKE ALL).
-- Returns counts only; never hashes, tokens, or event payloads.

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

CREATE FUNCTION fvoci.app_outbox_gc_processed(
    p_consumer text,
    p_window_days integer,
    p_limit integer
)
RETURNS integer
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    deleted integer;
    v_last_xact xid8;
    v_last_seq bigint;
    v_prev text;
BEGIN
    IF p_consumer IS NULL OR p_consumer !~ '^[a-z][a-z0-9_-]{0,62}$' THEN
        RAISE EXCEPTION 'invalid outbox consumer name';
    END IF;
    IF p_window_days IS NULL OR p_window_days < 1 THEN
        RAISE EXCEPTION 'app_outbox_gc_processed: window_days must be a positive integer';
    END IF;
    IF p_limit IS NULL OR p_limit < 1 THEN
        RAISE EXCEPTION 'app_outbox_gc_processed: limit must be a positive integer';
    END IF;

    v_prev := COALESCE(pg_catalog.current_setting('app.system_ctx', true), '');
    PERFORM pg_catalog.set_config('app.system_ctx', 'on', true);

    -- Source maxPos: highest (xact, seq) among this consumer's marks, joined
    -- through events because processed_events stores only event_id.
    SELECT e.xact, e.seq
    INTO v_last_xact, v_last_seq
    FROM fvoci.processed_events AS p
    INNER JOIN fvoci.events AS e ON e.id = p.event_id
    WHERE p.consumer = p_consumer
    ORDER BY e.xact DESC, e.seq DESC
    LIMIT 1;

    IF v_last_xact IS NULL THEN
        PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
        RETURN 0;
    END IF;

    WITH doomed AS (
        SELECT p.ctid
        FROM fvoci.processed_events AS p
        INNER JOIN fvoci.events AS e ON e.id = p.event_id
        WHERE p.consumer = p_consumer
          AND (e.xact, e.seq) < (v_last_xact, v_last_seq)
          AND p.processed_at < pg_catalog.now()
              - ((p_window_days::text || ' days')::interval)
        LIMIT p_limit
        FOR UPDATE OF p SKIP LOCKED
    )
    DELETE FROM fvoci.processed_events AS p
    USING doomed
    WHERE p.ctid = doomed.ctid;

    GET DIAGNOSTICS deleted = ROW_COUNT;
    PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
    RETURN deleted;
END;
$$;

REVOKE ALL ON FUNCTION fvoci.app_magic_purge_expired(timestamptz, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_gc_processed(text, integer, integer) FROM PUBLIC;
