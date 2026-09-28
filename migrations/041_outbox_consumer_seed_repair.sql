-- Repair consumer cursors that 018/020/027/040 left missing. Those seeds take
-- the newest event below the cluster-wide snapshot xmin, so they inserted no
-- row when a transaction in any database held xmin below every event, or when
-- the events came from a restored cluster whose xids are past this cluster's
-- xmax. The first server start would then create (0,0) and replay all history.
--
-- Only missing rows are added, and only when events exist: existing cursors
-- and fresh installs are unchanged. With tail = the newest (xact, seq), read in
-- the same statement as the snapshot bounds:
--   tail < xmin    every older xid has ended; start after tail (no replay, no
--                  skipped in-flight event).
--   tail >= xmax   restored from another cluster; start at tail. The relay stays
--                  fail-closed (epoch mismatch) until --recover-outbox rebases
--                  every cursor, these included, within its replay window.
--   otherwise      a transaction that may still commit earlier events is
--                  running; fail before seeding and let the operator rerun.
DO $$
DECLARE
    v_missing boolean;
    v_tail_xact xid8;
    v_tail_seq bigint;
    v_xmin xid8;
    v_xmax xid8;
BEGIN
    PERFORM pg_catalog.set_config('app.system_ctx', 'on', true);
    SELECT
        EXISTS (
            SELECT 1
            FROM (VALUES ('notifications'), ('mail'), ('webhooks'), ('github'), ('push')) AS c (name)
            WHERE NOT EXISTS (
                SELECT 1 FROM fvoci.outbox_consumers AS oc WHERE oc.consumer = c.name
            )
        ),
        t.xact,
        t.seq,
        pg_catalog.pg_snapshot_xmin(s.snap),
        pg_catalog.pg_snapshot_xmax(s.snap)
    INTO v_missing, v_tail_xact, v_tail_seq, v_xmin, v_xmax
    FROM (SELECT pg_catalog.pg_current_snapshot() AS snap) AS s
    LEFT JOIN LATERAL (
        SELECT ev.xact, ev.seq
        FROM fvoci.events AS ev
        ORDER BY ev.xact DESC, ev.seq DESC
        LIMIT 1
    ) AS t ON true;

    IF v_missing AND v_tail_xact IS NOT NULL THEN
        IF v_tail_xact >= v_xmin AND v_tail_xact < v_xmax THEN
            RAISE EXCEPTION 'outbox consumer seed repair: newest event xid % is not settled (snapshot xmin %)',
                v_tail_xact, v_xmin
                USING ERRCODE = 'object_not_in_prerequisite_state',
                      HINT = 'Wait for older transactions (pg_stat_activity, pg_prepared_xacts) to end, then rerun migrate.';
        END IF;
        INSERT INTO fvoci.outbox_consumers (consumer, last_xact, last_seq)
        SELECT c.name, v_tail_xact, v_tail_seq
        FROM (VALUES ('notifications'), ('mail'), ('webhooks'), ('github'), ('push')) AS c (name)
        ON CONFLICT (consumer) DO NOTHING;
    END IF;
    PERFORM pg_catalog.set_config('app.system_ctx', '', true);
END;
$$;
