CREATE TABLE fvoci.outbox_consumers (
    consumer text PRIMARY KEY,
    last_xact xid8 NOT NULL DEFAULT '0'::xid8,
    last_seq bigint NOT NULL DEFAULT 0,
    lease_owner uuid,
    lease_until timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT outbox_consumers_name_check CHECK (consumer ~ '^[a-z][a-z0-9_-]{0,62}$'),
    CONSTRAINT outbox_consumers_lease_pair_check
        CHECK ((lease_owner IS NULL) = (lease_until IS NULL))
);

CREATE TABLE fvoci.outbox_failures (
    consumer text NOT NULL,
    event_id uuid NOT NULL,
    attempts integer NOT NULL,
    last_error text NOT NULL,
    next_attempt_at timestamptz NOT NULL,
    dead_at timestamptz,
    skipped_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (consumer, event_id),
    CONSTRAINT outbox_failures_attempts_check CHECK (attempts > 0)
);

CREATE INDEX outbox_failures_retry_idx
    ON fvoci.outbox_failures (consumer, next_attempt_at)
    WHERE dead_at IS NULL;

CREATE TABLE fvoci.processed_events (
    consumer text NOT NULL,
    event_id uuid NOT NULL,
    processed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (consumer, event_id)
);

CREATE OR REPLACE FUNCTION fvoci.app_outbox_ensure_consumer(p_consumer text)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
BEGIN
    IF p_consumer IS NULL OR p_consumer !~ '^[a-z][a-z0-9_-]{0,62}$' THEN
        RAISE EXCEPTION 'invalid outbox consumer name';
    END IF;
    INSERT INTO fvoci.outbox_consumers (consumer)
    VALUES (p_consumer)
    ON CONFLICT (consumer) DO NOTHING;
END;
$$;

CREATE OR REPLACE FUNCTION fvoci.app_outbox_lease(
    p_consumer text,
    p_owner uuid,
    p_ttl_seconds integer DEFAULT 30
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    acquired boolean;
BEGIN
    PERFORM fvoci.app_outbox_ensure_consumer(p_consumer);

    UPDATE fvoci.outbox_consumers AS c
    SET
        lease_owner = p_owner,
        lease_until = pg_catalog.now() + make_interval(secs => p_ttl_seconds),
        updated_at = pg_catalog.now()
    WHERE c.consumer = p_consumer
      AND (
          c.lease_owner IS NULL
          OR c.lease_until < pg_catalog.now()
          OR c.lease_owner = p_owner
      )
    RETURNING true INTO acquired;

    RETURN COALESCE(acquired, false);
END;
$$;

CREATE OR REPLACE FUNCTION fvoci.app_outbox_release(
    p_consumer text,
    p_owner uuid
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    released boolean;
BEGIN
    UPDATE fvoci.outbox_consumers AS c
    SET
        lease_owner = NULL,
        lease_until = NULL,
        updated_at = pg_catalog.now()
    WHERE c.consumer = p_consumer
      AND c.lease_owner = p_owner
    RETURNING true INTO released;

    RETURN COALESCE(released, false);
END;
$$;

CREATE OR REPLACE FUNCTION fvoci.app_outbox_read(
    p_consumer text,
    p_limit integer DEFAULT 100
)
RETURNS TABLE (
    snapshot_xmin xid8,
    event_id uuid,
    seq bigint,
    xact xid8,
    workspace_id uuid,
    actor_user_id uuid,
    verb text,
    target_type text,
    target_id uuid,
    payload jsonb,
    channel text,
    created_at timestamptz
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    v_xmin xid8;
    v_last_xact xid8;
    v_last_seq bigint;
    v_prev text;
BEGIN
    v_prev := COALESCE(pg_catalog.current_setting('app.system_ctx', true), '');
    PERFORM pg_catalog.set_config('app.system_ctx', 'on', true);

    SELECT pg_catalog.pg_snapshot_xmin(pg_catalog.pg_current_snapshot()) INTO v_xmin;

    SELECT c.last_xact, c.last_seq
    INTO v_last_xact, v_last_seq
    FROM fvoci.outbox_consumers AS c
    WHERE c.consumer = p_consumer;

    IF NOT FOUND THEN
        PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
        RETURN;
    END IF;

    -- Only xids from the future (at or past xmax) mean another cluster's epoch;
    -- a cursor at or above xmin is still waiting on running transactions.
    -- Snapshot and comparison must share one statement (READ COMMITTED).
    IF EXISTS (
            SELECT 1
            FROM fvoci.outbox_consumers AS c
            WHERE c.consumer = p_consumer
              AND c.last_xact >= pg_catalog.pg_snapshot_xmax(pg_catalog.pg_current_snapshot())
       )
       OR EXISTS (
            SELECT 1
            FROM fvoci.events AS e
            WHERE e.xact >= pg_catalog.pg_snapshot_xmax(pg_catalog.pg_current_snapshot())
       )
    THEN
        PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
        RAISE EXCEPTION 'outbox xid epoch mismatch; run fvoci-migrate --recover-outbox'
            USING ERRCODE = 'data_exception';
    END IF;

    RETURN QUERY
    SELECT
        v_xmin,
        e.id,
        e.seq,
        e.xact,
        e.workspace_id,
        e.actor_user_id,
        e.verb,
        e.target_type,
        e.target_id,
        e.payload,
        e.channel,
        e.created_at
    FROM fvoci.events AS e
    WHERE (e.xact, e.seq) > (v_last_xact, v_last_seq)
      AND e.xact < v_xmin
    ORDER BY e.xact, e.seq
    LIMIT GREATEST(p_limit, 0);

    PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
END;
$$;

CREATE OR REPLACE FUNCTION fvoci.app_outbox_advance(
    p_consumer text,
    p_owner uuid,
    p_xact xid8,
    p_seq bigint
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    advanced boolean;
    already_applied boolean;
    v_xmin xid8;
    v_prev text;
    v_event_id uuid;
BEGIN
    v_prev := COALESCE(pg_catalog.current_setting('app.system_ctx', true), '');
    PERFORM pg_catalog.set_config('app.system_ctx', 'on', true);
    SELECT pg_catalog.pg_snapshot_xmin(pg_catalog.pg_current_snapshot()) INTO v_xmin;

    -- read() already fails closed on events >= xmax; refuse still-running xids.
    IF p_xact >= v_xmin THEN
        PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
        RETURN false;
    END IF;

    SELECT e.id
    INTO v_event_id
    FROM fvoci.events AS e
    WHERE e.xact = p_xact AND e.seq = p_seq;

    IF v_event_id IS NULL THEN
        PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
        RETURN false;
    END IF;

    -- Retry at or below the cursor is only a dead-letter skip that was requeued.
    SELECT true
    INTO already_applied
    FROM fvoci.outbox_consumers AS c
    INNER JOIN fvoci.outbox_failures AS f
        ON f.consumer = c.consumer
       AND f.event_id = v_event_id
    WHERE c.consumer = p_consumer
      AND c.lease_owner = p_owner
      AND c.lease_until > pg_catalog.now()
      AND (p_xact, p_seq) <= (c.last_xact, c.last_seq)
      AND f.skipped_at IS NOT NULL
      AND f.dead_at IS NULL;

    IF already_applied THEN
        DELETE FROM fvoci.outbox_failures AS f
        WHERE f.consumer = p_consumer
          AND f.event_id = v_event_id
          AND f.skipped_at IS NOT NULL
          AND f.dead_at IS NULL;
        PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
        RETURN true;
    END IF;

    UPDATE fvoci.outbox_consumers AS c
    SET
        last_xact = p_xact,
        last_seq = p_seq,
        updated_at = pg_catalog.now()
    WHERE c.consumer = p_consumer
      AND c.lease_owner = p_owner
      AND c.lease_until > pg_catalog.now()
      AND c.last_xact < v_xmin
      AND (p_xact, p_seq) > (c.last_xact, c.last_seq)
    RETURNING true INTO advanced;

    IF COALESCE(advanced, false) THEN
        UPDATE fvoci.outbox_failures AS f
        SET
            skipped_at = COALESCE(f.skipped_at, pg_catalog.now()),
            updated_at = pg_catalog.now()
        WHERE f.consumer = p_consumer
          AND f.event_id = v_event_id
          AND f.dead_at IS NOT NULL;
    END IF;

    PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
    RETURN COALESCE(advanced, false);
END;
$$;

CREATE OR REPLACE FUNCTION fvoci.app_outbox_record_failure(
    p_consumer text,
    p_owner uuid,
    p_event_id uuid,
    p_error text,
    p_backoff_ms integer DEFAULT 1000,
    p_max_attempts integer DEFAULT 5
)
RETURNS integer
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    v_attempts integer;
    v_base_ms integer;
    v_max_attempts integer;
    v_dead timestamptz;
    v_prev text;
    v_leased boolean;
    v_at_or_below boolean;
BEGIN
    v_prev := COALESCE(pg_catalog.current_setting('app.system_ctx', true), '');
    PERFORM pg_catalog.set_config('app.system_ctx', 'on', true);
    v_base_ms := GREATEST(p_backoff_ms, 1);
    v_max_attempts := GREATEST(p_max_attempts, 1);

    SELECT true
    INTO v_leased
    FROM fvoci.outbox_consumers AS c
    WHERE c.consumer = p_consumer
      AND c.lease_owner = p_owner
      AND c.lease_until > pg_catalog.now();

    IF NOT COALESCE(v_leased, false) THEN
        PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
        RETURN 0;
    END IF;

    SELECT f.attempts, f.dead_at
    INTO v_attempts, v_dead
    FROM fvoci.outbox_failures AS f
    WHERE f.consumer = p_consumer AND f.event_id = p_event_id;

    IF v_dead IS NOT NULL THEN
        PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
        RETURN v_attempts;
    END IF;

    -- Do not create a live failure for an event the cursor has already passed.
    IF v_attempts IS NULL THEN
        SELECT true
        INTO v_at_or_below
        FROM fvoci.outbox_consumers AS c
        INNER JOIN fvoci.events AS e ON e.id = p_event_id
        WHERE c.consumer = p_consumer
          AND (e.xact, e.seq) <= (c.last_xact, c.last_seq);

        IF COALESCE(v_at_or_below, false) THEN
            PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
            RETURN 0;
        END IF;
    END IF;

    INSERT INTO fvoci.outbox_failures AS f (
        consumer,
        event_id,
        attempts,
        last_error,
        next_attempt_at,
        dead_at
    ) VALUES (
        p_consumer,
        p_event_id,
        1,
        left(p_error, 4000),
        pg_catalog.now() + ((v_base_ms::text || ' milliseconds')::interval),
        CASE WHEN v_max_attempts <= 1 THEN pg_catalog.now() ELSE NULL END
    )
    ON CONFLICT (consumer, event_id) DO UPDATE
    SET
        attempts = f.attempts + 1,
        last_error = EXCLUDED.last_error,
        next_attempt_at = pg_catalog.now() + (
            (
                LEAST(
                    60000::bigint,
                    v_base_ms::bigint * (2 ^ LEAST(f.attempts, 20))::bigint
                )::text || ' milliseconds'
            )::interval
        ),
        dead_at = CASE
            WHEN f.attempts + 1 >= v_max_attempts THEN pg_catalog.now()
            ELSE NULL
        END,
        updated_at = pg_catalog.now()
    RETURNING attempts INTO v_attempts;

    PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
    RETURN v_attempts;
END;
$$;

CREATE OR REPLACE FUNCTION fvoci.app_outbox_clear_failure(
    p_consumer text,
    p_event_id uuid
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    cleared boolean;
BEGIN
    DELETE FROM fvoci.outbox_failures AS f
    WHERE f.consumer = p_consumer
      AND f.event_id = p_event_id
    RETURNING true INTO cleared;

    RETURN COALESCE(cleared, false);
END;
$$;

CREATE OR REPLACE FUNCTION fvoci.app_outbox_failure_state(
    p_consumer text,
    p_event_id uuid
)
RETURNS TABLE (
    attempts integer,
    next_attempt_at timestamptz,
    dead_at timestamptz
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
BEGIN
    RETURN QUERY
    SELECT f.attempts, f.next_attempt_at, f.dead_at
    FROM fvoci.outbox_failures AS f
    WHERE f.consumer = p_consumer
      AND f.event_id = p_event_id;
END;
$$;

CREATE OR REPLACE FUNCTION fvoci.app_outbox_requeue(
    p_consumer text,
    p_event_id uuid
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    requeued boolean;
BEGIN
    UPDATE fvoci.outbox_failures AS f
    SET
        attempts = 1,
        dead_at = NULL,
        next_attempt_at = pg_catalog.now(),
        updated_at = pg_catalog.now()
    WHERE f.consumer = p_consumer
      AND f.event_id = p_event_id
    RETURNING true INTO requeued;

    RETURN COALESCE(requeued, false);
END;
$$;

CREATE OR REPLACE FUNCTION fvoci.app_outbox_claim_retries(
    p_consumer text,
    p_limit integer DEFAULT 50
)
RETURNS TABLE (
    event_id uuid,
    attempts integer,
    last_error text
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    v_prev text;
BEGIN
    v_prev := COALESCE(pg_catalog.current_setting('app.system_ctx', true), '');
    PERFORM pg_catalog.set_config('app.system_ctx', 'on', true);

    RETURN QUERY
    SELECT f.event_id, f.attempts, f.last_error
    FROM fvoci.outbox_failures AS f
    INNER JOIN fvoci.outbox_consumers AS c ON c.consumer = f.consumer
    INNER JOIN fvoci.events AS e ON e.id = f.event_id
    WHERE f.consumer = p_consumer
      AND f.dead_at IS NULL
      AND f.skipped_at IS NOT NULL
      AND f.next_attempt_at <= pg_catalog.now()
      AND (e.xact, e.seq) <= (c.last_xact, c.last_seq)
    ORDER BY f.next_attempt_at, f.event_id
    LIMIT GREATEST(p_limit, 0);

    PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
END;
$$;

CREATE OR REPLACE FUNCTION fvoci.app_outbox_mark_processed(
    p_consumer text,
    p_event_id uuid
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    inserted boolean;
BEGIN
    INSERT INTO fvoci.processed_events (consumer, event_id)
    VALUES (p_consumer, p_event_id)
    ON CONFLICT DO NOTHING
    RETURNING true INTO inserted;

    RETURN COALESCE(inserted, false);
END;
$$;

CREATE OR REPLACE FUNCTION fvoci.app_outbox_is_processed(
    p_consumer text,
    p_event_id uuid
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
BEGIN
    RETURN EXISTS (
        SELECT 1
        FROM fvoci.processed_events AS p
        WHERE p.consumer = p_consumer
          AND p.event_id = p_event_id
    );
END;
$$;

REVOKE ALL ON FUNCTION fvoci.app_outbox_ensure_consumer(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_lease(text, uuid, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_release(text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_read(text, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_advance(text, uuid, xid8, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_record_failure(text, uuid, uuid, text, integer, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_clear_failure(text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_failure_state(text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_requeue(text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_claim_retries(text, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_mark_processed(text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_is_processed(text, uuid) FROM PUBLIC;
