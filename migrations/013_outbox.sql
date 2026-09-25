CREATE TABLE fvoci.outbox_consumers (
    consumer text PRIMARY KEY,
    last_xact xid8 NOT NULL DEFAULT '0'::xid8,
    last_seq bigint NOT NULL DEFAULT 0,
    lease_owner uuid,
    lease_until timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT outbox_consumers_lease_pair_check
        CHECK ((lease_owner IS NULL) = (lease_until IS NULL))
);

CREATE TABLE fvoci.outbox_failures (
    consumer text NOT NULL,
    event_id uuid NOT NULL,
    attempts integer NOT NULL,
    last_error text NOT NULL,
    next_attempt_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (consumer, event_id),
    CONSTRAINT outbox_failures_attempts_check CHECK (attempts > 0)
);

CREATE INDEX outbox_failures_retry_idx
    ON fvoci.outbox_failures (consumer, next_attempt_at);

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
BEGIN
    SELECT pg_catalog.pg_snapshot_xmin(pg_catalog.pg_current_snapshot()) INTO v_xmin;

    SELECT c.last_xact, c.last_seq
    INTO v_last_xact, v_last_seq
    FROM fvoci.outbox_consumers AS c
    WHERE c.consumer = p_consumer;

    IF NOT FOUND THEN
        RETURN;
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
BEGIN
    UPDATE fvoci.outbox_consumers AS c
    SET
        last_xact = p_xact,
        last_seq = p_seq,
        updated_at = pg_catalog.now()
    WHERE c.consumer = p_consumer
      AND c.lease_owner = p_owner
      AND c.lease_until > pg_catalog.now()
      AND (p_xact, p_seq) > (c.last_xact, c.last_seq)
    RETURNING true INTO advanced;

    RETURN COALESCE(advanced, false);
END;
$$;

CREATE OR REPLACE FUNCTION fvoci.app_outbox_record_failure(
    p_consumer text,
    p_event_id uuid,
    p_error text,
    p_backoff_seconds integer DEFAULT 60
)
RETURNS integer
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    v_attempts integer;
BEGIN
    INSERT INTO fvoci.outbox_failures AS f (
        consumer,
        event_id,
        attempts,
        last_error,
        next_attempt_at
    ) VALUES (
        p_consumer,
        p_event_id,
        1,
        left(p_error, 4000),
        pg_catalog.now() + make_interval(secs => GREATEST(p_backoff_seconds, 1))
    )
    ON CONFLICT (consumer, event_id) DO UPDATE
    SET
        attempts = f.attempts + 1,
        last_error = EXCLUDED.last_error,
        next_attempt_at = EXCLUDED.next_attempt_at,
        updated_at = pg_catalog.now()
    RETURNING attempts INTO v_attempts;

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
BEGIN
    RETURN QUERY
    SELECT f.event_id, f.attempts, f.last_error
    FROM fvoci.outbox_failures AS f
    WHERE f.consumer = p_consumer
      AND f.next_attempt_at <= pg_catalog.now()
    ORDER BY f.next_attempt_at, f.event_id
    LIMIT GREATEST(p_limit, 0);
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
REVOKE ALL ON FUNCTION fvoci.app_outbox_record_failure(text, uuid, text, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_clear_failure(text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_claim_retries(text, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_mark_processed(text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_is_processed(text, uuid) FROM PUBLIC;
