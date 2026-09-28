-- Workspace event log paging and outbox lag probes.
--
-- 1. GET /workspaces/{id}/events reads one workspace in (xact, seq) order
--    after a cursor. events_relay_idx (xact, seq) walks every tenant's rows and
--    events_workspace_idx (workspace_id, created_at) sorts the whole workspace;
--    (workspace_id, xact, seq) gives an ordered range scan. Migrations run in
--    one transaction, so this is a plain CREATE INDEX: it holds a SHARE lock
--    on fvoci.events (reads continue, event inserts wait) while it builds.
CREATE INDEX events_workspace_relay_idx ON fvoci.events (workspace_id, xact, seq);

-- 2. Outbox lag for /metrics, as the source lagSeconds(): the age of the
--    oldest event (by created_at) past a consumer cursor, maximised over the
--    given consumers. Unlike app_outbox_read there is no snapshot-xmin filter,
--    so events committed behind a long-running transaction (xmin stall) count.
--    Unknown consumers and consumers with no backlog contribute 0. Returns
--    only a number of seconds.
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

-- 3. Age of the oldest transaction holding an xid in this cluster, as the
--    source oldestWriteTxAgeMs(): the one that pins the snapshot xmin every
--    outbox consumer waits on. Prepared transactions hold an xid without a
--    backend and count too. The caller's own backend is excluded. Returns
--    only seconds (0 when none): no pid, role, database or query text.
--    pg_stat_activity shows xact_start/backend_xid of other roles' sessions
--    only to a superuser or pg_read_all_stats member, so this is SECURITY
--    DEFINER; with a less privileged owner it covers the sessions the owner
--    may see plus every prepared transaction.
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

REVOKE ALL ON FUNCTION fvoci.app_outbox_lag_seconds(text[]) FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_oldest_write_xact_age_seconds() FROM PUBLIC;
