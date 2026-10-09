-- FVOCI PostgreSQL baseline fvoci-postgres-060, step 04: events and outbox.
-- The append-only event log, the audit log, and the outbox relay ledger
-- (consumer cursors, failures, processed marks) with its SECURITY DEFINER API.
-- A fresh install has no consumer rows: fvoci.app_outbox_ensure_consumer creates
-- a cursor at (0, 0) when a consumer first leases, and relays every event.

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
-- GET /workspaces/{id}/events reads one workspace in (xact, seq) order after a
-- cursor: an ordered range scan per tenant.
CREATE INDEX events_workspace_relay_idx ON fvoci.events (workspace_id, xact, seq);

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

ALTER TABLE fvoci.events ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.events FORCE ROW LEVEL SECURITY;
ALTER TABLE fvoci.audit_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.audit_log FORCE ROW LEVEL SECURITY;

CREATE POLICY events_insert ON fvoci.events
    FOR INSERT WITH CHECK (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

CREATE POLICY events_select ON fvoci.events
    FOR SELECT USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

CREATE POLICY audit_log_select ON fvoci.audit_log
    FOR SELECT USING ((SELECT public.app_system_ctx_on()));

CREATE POLICY audit_log_insert ON fvoci.audit_log
    FOR INSERT WITH CHECK (true);

-- Outbox relay ledger. The app role has no table privileges on these three
-- tables; every access goes through the definers below.
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

CREATE FUNCTION fvoci.app_outbox_ensure_consumer(p_consumer text)
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

CREATE FUNCTION fvoci.app_outbox_lease(
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

CREATE FUNCTION fvoci.app_outbox_release(
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

CREATE FUNCTION fvoci.app_outbox_read(
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

CREATE FUNCTION fvoci.app_outbox_advance(
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
    -- Lock the lease row like the forward UPDATE does, so a lease steal waits for
    -- this transaction, and let the DELETE itself be the check: a concurrent owner
    -- that already resolved the row deletes nothing and must roll back.
    PERFORM 1
    FROM fvoci.outbox_consumers AS c
    WHERE c.consumer = p_consumer
      AND c.lease_owner = p_owner
      AND c.lease_until > pg_catalog.now()
    FOR NO KEY UPDATE;

    DELETE FROM fvoci.outbox_failures AS f
    USING fvoci.outbox_consumers AS c
    WHERE f.consumer = p_consumer
      AND f.event_id = v_event_id
      AND f.skipped_at IS NOT NULL
      AND f.dead_at IS NULL
      AND c.consumer = f.consumer
      AND c.lease_owner = p_owner
      AND c.lease_until > pg_catalog.now()
      AND (p_xact, p_seq) <= (c.last_xact, c.last_seq)
    RETURNING true INTO already_applied;

    IF already_applied THEN
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

CREATE FUNCTION fvoci.app_outbox_record_failure(
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

CREATE FUNCTION fvoci.app_outbox_clear_failure(
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

CREATE FUNCTION fvoci.app_outbox_failure_state(
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

CREATE FUNCTION fvoci.app_outbox_requeue(
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
    -- Only a row the dispatcher already skipped past; a dead row still above the
    -- cursor is skipped on the next pass and can be requeued after that.
    WHERE f.consumer = p_consumer
      AND f.event_id = p_event_id
      AND f.skipped_at IS NOT NULL
    RETURNING true INTO requeued;

    RETURN COALESCE(requeued, false);
END;
$$;

CREATE FUNCTION fvoci.app_outbox_claim_retries(
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

CREATE FUNCTION fvoci.app_outbox_mark_processed(
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

CREATE FUNCTION fvoci.app_outbox_is_processed(
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

-- Bounded GC of processed marks below the consumer's newest mark.
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

-- Outbox lag for /metrics, as the source lagSeconds(): the age of the oldest
-- event (by created_at) past a consumer cursor, maximised over the given
-- consumers. Unlike app_outbox_read there is no snapshot-xmin filter, so events
-- committed behind a long-running transaction (xmin stall) count. Unknown
-- consumers and consumers with no backlog contribute 0. Returns only seconds.
CREATE FUNCTION fvoci.app_outbox_lag_seconds(p_consumers text[])
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    v_prev text;
    v_lag bigint;
BEGIN
    v_prev := COALESCE(pg_catalog.current_setting('app.system_ctx', true), '');
    PERFORM pg_catalog.set_config('app.system_ctx', 'on', true);

    SELECT COALESCE(pg_catalog.max(
        GREATEST(EXTRACT(EPOCH FROM (pg_catalog.now() - oldest.created_at)), 0)
    ), 0)::bigint
    INTO v_lag
    FROM fvoci.outbox_consumers AS c
    CROSS JOIN LATERAL (
        SELECT pg_catalog.min(e.created_at) AS created_at
        FROM fvoci.events AS e
        WHERE (e.xact, e.seq) > (c.last_xact, c.last_seq)
    ) AS oldest
    WHERE c.consumer = ANY (p_consumers);

    PERFORM pg_catalog.set_config('app.system_ctx', v_prev, true);
    RETURN v_lag;
END;
$$;

-- Age of the oldest transaction holding an xid in this cluster, as the source
-- oldestWriteTxAgeMs(): the one that pins the snapshot xmin every outbox
-- consumer waits on. Prepared transactions hold an xid without a backend and
-- count too. The caller's own backend is excluded. Returns only seconds (0 when
-- none): no pid, role, database or query text. pg_stat_activity shows
-- xact_start/backend_xid of other roles' sessions only to a superuser or
-- pg_read_all_stats member, so this is SECURITY DEFINER; with a less privileged
-- owner it covers the sessions the owner may see plus every prepared transaction.
CREATE FUNCTION fvoci.app_oldest_write_xact_age_seconds()
RETURNS bigint
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
    SELECT COALESCE(pg_catalog.max(GREATEST(EXTRACT(EPOCH FROM (pg_catalog.now() - started)), 0)), 0)::bigint
    FROM (
        SELECT a.xact_start AS started
        FROM pg_catalog.pg_stat_activity AS a
        WHERE a.backend_xid IS NOT NULL
          AND a.pid <> pg_catalog.pg_backend_pid()
        UNION ALL
        SELECT p.prepared
        FROM pg_catalog.pg_prepared_xacts AS p
    ) AS holders;
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
REVOKE ALL ON FUNCTION fvoci.app_outbox_gc_processed(text, integer, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_outbox_lag_seconds(text[]) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_oldest_write_xact_age_seconds() FROM PUBLIC;
