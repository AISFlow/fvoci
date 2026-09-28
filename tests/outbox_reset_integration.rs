#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! `fvoci-migrate --outbox-reset` (source `fvoci outbox-reset`) against a real
//! PostgreSQL, run as the binary with the owner `DATABASE_URL` it documents.
//! The consumers that resume afterwards use the NOSUPERUSER/NOBYPASSRLS app
//! role, as the server does.

#[path = "support/project_harness.rs"]
mod project_harness;

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use fvoci_server::db::outbox::{
    advance_cursor_tx, ensure_consumer, fetch_cursor, insert_test_event, lease_consumer,
    mark_processed, mark_processed_tx, OutboxEvent,
};
use fvoci_server::db::pool;
use fvoci_server::outbox::{
    spawn_outbox_dispatcher, DeliveryMode, OutboxConsumer, OutboxDispatcherHandle,
    OutboxDispatcherSettings, OutboxProcessError,
};
use project_harness::{close_pool, TestDb};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgPool, Row};
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(15);

struct Run {
    ok: bool,
    report: Value,
    output: String,
}

async fn run(harness: &TestDb, args: &[&str]) -> Run {
    run_as(&harness.admin_url, args).await
}

async fn run_as(url: &str, args: &[&str]) -> Run {
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_fvoci-migrate"))
        .arg("--outbox-reset")
        .args(args)
        .env_clear()
        .env("DATABASE_URL", url)
        .output()
        .await
        .expect("run fvoci-migrate");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    Run {
        ok: out.status.success(),
        report: serde_json::from_str(stdout.trim()).unwrap_or(Value::Null),
        output: format!("{stdout}{stderr}"),
    }
}

fn db_name(harness: &TestDb) -> String {
    url::Url::parse(&harness.admin_url)
        .unwrap()
        .path()
        .trim_start_matches('/')
        .to_string()
}

fn app_role(harness: &TestDb) -> String {
    url::Url::parse(&harness.app_url)
        .unwrap()
        .username()
        .to_string()
}

async fn admin(harness: &TestDb) -> PgPool {
    PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .expect("admin")
}

async fn app(harness: &TestDb) -> PgPool {
    pool::connect_app(&harness.app_url).await.expect("app")
}

/// Polls `probe` until it reports `Ok`; on timeout the panic names the wait
/// and the last state the probe observed.
async fn wait_until<F>(what: &str, mut probe: F)
where
    F: FnMut() -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
{
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let last = match probe().await {
            Ok(()) => return,
            Err(state) => state,
        };
        if std::time::Instant::now() >= deadline {
            panic!("{what}: not met within {WAIT:?}; last observed: {last}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// (datname, pid, backend_xid, backend_xmin) from `pg_stat_activity`.
type XidHolder = (Option<String>, i32, Option<String>, Option<String>);

/// Cluster snapshot xmin and its oldest holders, for timeout reports.
async fn xmin_state(pool: &PgPool) -> String {
    let xmin: String = sqlx::query_scalar("SELECT pg_snapshot_xmin(pg_current_snapshot())::text")
        .fetch_one(pool)
        .await
        .unwrap_or_else(|err| err.to_string());
    let holders: Vec<XidHolder> = sqlx::query_as(
        "SELECT datname::text, pid, backend_xid::text, backend_xmin::text \
         FROM pg_stat_activity \
         WHERE backend_xid IS NOT NULL OR backend_xmin IS NOT NULL \
         ORDER BY age(COALESCE(backend_xid, backend_xmin)) DESC LIMIT 5",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    format!("snapshot xmin {xmin}, oldest (datname, pid, xid, xmin) {holders:?}")
}

/// xmin is cluster-wide: a transaction in any database (another test's, a
/// parallel binary's) holds it below events this test already committed.
/// The relay reads only events below xmin and a forward `--apply` refuses a
/// target at or above it, so wait until every transaction older than now has
/// ended. The horizon is the current xmax, so the wait assigns no xid.
async fn wait_events_settled(pool: &PgPool) {
    let horizon: String =
        sqlx::query_scalar("SELECT pg_snapshot_xmax(pg_current_snapshot())::text")
            .fetch_one(pool)
            .await
            .expect("snapshot xmax");
    let what = format!("cluster snapshot xmin at or past {horizon}");
    wait_until(&what, || {
        let pool = pool.clone();
        let horizon = horizon.clone();
        Box::pin(async move {
            let settled: bool =
                sqlx::query_scalar("SELECT pg_snapshot_xmin(pg_current_snapshot()) >= $1::xid8")
                    .bind(horizon)
                    .fetch_one(&pool)
                    .await
                    .map_err(|err| err.to_string())?;
            if settled {
                Ok(())
            } else {
                Err(xmin_state(&pool).await)
            }
        })
    })
    .await;
}

/// `--apply` refuses while any other session is connected, so the test
/// closes its pools and waits until the server has let the backends go.
async fn wait_for_no_sessions(harness: &TestDb) {
    let observer = server_observer(harness).await;
    no_sessions(harness, &observer).await;
    observer.close().await;
}

/// As `wait_for_no_sessions`, and the test's committed events are settled, so
/// a forward `--apply` is not refused for a transaction elsewhere in the
/// cluster.
async fn wait_for_settled_apply(harness: &TestDb) {
    let observer = server_observer(harness).await;
    wait_events_settled(&observer).await;
    no_sessions(harness, &observer).await;
    observer.close().await;
}

/// A session in the `postgres` database: it is not one the apply counts.
async fn server_observer(harness: &TestDb) -> PgPool {
    let mut server = url::Url::parse(&harness.admin_url).unwrap();
    server.set_path("/postgres");
    PgPoolOptions::new()
        .max_connections(1)
        .connect(server.as_str())
        .await
        .expect("observer")
}

async fn no_sessions(harness: &TestDb, observer: &PgPool) {
    let name = db_name(harness);
    wait_until("no other session in the test database", || {
        let pool = observer.clone();
        let name = name.clone();
        Box::pin(async move {
            let sessions: Vec<(i32, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT pid, usename::text, state FROM pg_stat_activity WHERE datname = $1 AND backend_type = 'client backend'",
            )
            .bind(name)
            .fetch_all(&pool)
            .await
            .map_err(|err| err.to_string())?;
            if sessions.is_empty() {
                Ok(())
            } else {
                Err(format!("sessions (pid, user, state) {sessions:?}"))
            }
        })
    })
    .await;
}

async fn cursors(admin: &PgPool) -> Vec<(String, String, i64)> {
    sqlx::query("SELECT consumer, last_xact::text AS x, last_seq FROM fvoci.outbox_consumers ORDER BY consumer")
        .fetch_all(admin)
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get("consumer"), r.get("x"), r.get("last_seq")))
        .collect()
}

async fn counts(admin: &PgPool) -> (i64, i64, i64) {
    let row = sqlx::query(
        r#"
        SELECT (SELECT count(*) FROM fvoci.events) AS events,
               (SELECT count(*) FROM fvoci.processed_events) AS marks,
               (SELECT count(*) FROM fvoci.outbox_failures) AS failures
        "#,
    )
    .fetch_one(admin)
    .await
    .unwrap();
    (row.get("events"), row.get("marks"), row.get("failures"))
}

async fn event_pos(admin: &PgPool, id: Uuid) -> (String, i64) {
    let row = sqlx::query("SELECT xact::text AS x, seq FROM fvoci.events WHERE id = $1")
        .bind(id)
        .fetch_one(admin)
        .await
        .unwrap();
    (row.get("x"), row.get("seq"))
}

fn consumer<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["consumers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["consumer"] == name)
        .unwrap_or_else(|| panic!("{name} missing from {report}"))
}

fn names(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|c| c["consumer"].as_str().unwrap().to_string())
        .collect()
}

async fn install_delivery_table(admin: &PgPool, role: &str) {
    sqlx::query(
        r#"
        CREATE TABLE fvoci.outbox_reset_deliveries (
            consumer text NOT NULL,
            event_id uuid NOT NULL,
            PRIMARY KEY (consumer, event_id)
        )
        "#,
    )
    .execute(admin)
    .await
    .unwrap();
    sqlx::query(&format!(
        "GRANT SELECT, INSERT ON fvoci.outbox_reset_deliveries TO \"{}\"",
        role.replace('"', "\"\"")
    ))
    .execute(admin)
    .await
    .unwrap();
}

/// A PgOnly consumer shaped like the notifications consumer: mark, effect and
/// advance in one transaction. The effect is a plain INSERT under a primary
/// key, so a second delivery of one event fails instead of hiding.
struct RecordingConsumer(&'static str);

impl OutboxConsumer for RecordingConsumer {
    fn name(&self) -> &str {
        self.0
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::PgOnly
    }

    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            let mut tx = pool.begin().await?;
            if mark_processed_tx(&mut tx, self.0, event.id).await? {
                sqlx::query(
                    "INSERT INTO fvoci.outbox_reset_deliveries (consumer, event_id) VALUES ($1, $2)",
                )
                .bind(self.0)
                .bind(event.id)
                .execute(&mut *tx)
                .await?;
            }
            if !advance_cursor_tx(&mut tx, self.0, lease_owner, &event.xact, event.seq).await? {
                tx.rollback().await?;
                return Err(OutboxProcessError::Delivery("advance rejected".into()));
            }
            tx.commit().await?;
            Ok(())
        })
    }
}

fn dispatcher(pool: PgPool, name: &'static str) -> OutboxDispatcherHandle {
    spawn_outbox_dispatcher(
        OutboxDispatcherSettings {
            poll_interval: Duration::from_millis(20),
            lease_ttl: Duration::from_secs(2),
            batch_limit: 50,
            failure_backoff: Duration::from_millis(50),
        },
        pool,
        vec![Arc::new(RecordingConsumer(name))],
    )
    .expect("dispatcher")
}

async fn deliveries(pool: &PgPool, name: &str) -> Vec<Uuid> {
    sqlx::query_scalar(
        "SELECT event_id FROM fvoci.outbox_reset_deliveries WHERE consumer = $1 ORDER BY event_id",
    )
    .bind(name)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// `Ok` once `name` has recorded `want` deliveries; otherwise the state that
/// explains why not: cursor, lease, failures and the cluster xmin.
async fn delivered(admin: &PgPool, name: &str, want: usize) -> Result<(), String> {
    let got = deliveries(admin, name).await.len();
    if got == want {
        return Ok(());
    }
    let cursor: Option<(String, i64, Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT last_xact::text, last_seq, lease_owner, lease_until::text FROM fvoci.outbox_consumers WHERE consumer = $1",
    )
    .bind(name)
    .fetch_optional(admin)
    .await
    .map_err(|err| err.to_string())?;
    let failures: Vec<(Uuid, i32, bool, bool, String)> = sqlx::query_as(
        "SELECT event_id, attempts, dead_at IS NOT NULL, skipped_at IS NOT NULL, last_error FROM fvoci.outbox_failures WHERE consumer = $1",
    )
    .bind(name)
    .fetch_all(admin)
    .await
    .map_err(|err| err.to_string())?;
    let events: Vec<(String, i64)> =
        sqlx::query_as("SELECT xact::text, seq FROM fvoci.events ORDER BY xact, seq")
            .fetch_all(admin)
            .await
            .map_err(|err| err.to_string())?;
    Err(format!(
        "{got}/{want} deliveries; cursor (xact, seq, lease owner, until) {cursor:?}; failures (event, attempts, dead, skipped, error) {failures:?}; events {events:?}; {}",
        xmin_state(admin).await
    ))
}

async fn stop(handle: OutboxDispatcherHandle) {
    handle.request_shutdown();
    handle.join().await.expect("dispatcher join");
}

#[tokio::test]
async fn diagnose_is_read_only_while_live_and_github_is_opt_in() {
    let harness = TestDb::bootstrap().await;
    let admin = admin(&harness).await;
    let app = app(&harness).await;
    for name in ["notifications", "mail", "github", "search-index"] {
        ensure_consumer(&app, name).await.unwrap();
    }
    let mut ids = Vec::new();
    for n in 0..3 {
        ids.push(
            insert_test_event(&app, "test.reset", json!({ "n": n }))
                .await
                .unwrap(),
        );
    }
    for id in &ids[..2] {
        mark_processed(&app, "notifications", *id).await.unwrap();
        mark_processed(&app, "github", *id).await.unwrap();
    }
    let before = cursors(&admin).await;
    let totals = counts(&admin).await;

    // The server is "live" (pools connected): diagnose still runs, writes nothing.
    let diag = run(&harness, &[]).await;
    assert!(diag.ok, "{}", diag.output);
    assert_eq!(diag.report["mode"], "diagnose");
    assert_eq!(diag.report["applied"], false);
    assert_eq!(diag.report["windowDays"], 29);
    assert_eq!(
        names(&diag.report["consumers"]),
        vec!["mail", "notifications"]
    );
    assert_eq!(
        names(&diag.report["excluded"]),
        vec!["github", "search-index"]
    );
    let notifications = consumer(&diag.report, "notifications");
    let (x2, s2) = event_pos(&admin, ids[1]).await;
    assert_eq!(notifications["direction"], "forward");
    assert_eq!(notifications["target"]["lastXact"], x2.as_str());
    assert_eq!(notifications["target"]["lastSeq"], s2);
    assert_eq!(notifications["skip"], Value::Null);
    // mail marked nothing: the event before the first unmarked one is the origin.
    assert_eq!(consumer(&diag.report, "mail")["direction"], "unchanged");
    assert_eq!(cursors(&admin).await, before);
    assert_eq!(counts(&admin).await, totals);

    let named = run(&harness, &["--consumer", "github"]).await;
    assert!(named.ok, "{}", named.output);
    assert_eq!(names(&named.report["consumers"]), vec!["github"]);
    assert_eq!(consumer(&named.report, "github")["direction"], "forward");
    assert!(named.report["excluded"].as_array().unwrap().is_empty());

    let unknown = run(&harness, &["--consumer", "nobody"]).await;
    assert!(!unknown.ok);
    assert!(
        unknown.output.contains("unknown outbox consumer"),
        "{}",
        unknown.output
    );

    // Apply while other sessions are connected is refused without writing.
    let live = run(&harness, &["--apply", "--reason", "ticket-1"]).await;
    assert!(!live.ok);
    assert!(live.output.contains("stop the server"), "{}", live.output);
    assert_eq!(cursors(&admin).await, before);

    for bad in [
        &["--apply"][..],
        &["--apply", "--reason="],
        &["--override-reason=x"],
    ] {
        let refused = run(&harness, bad).await;
        assert!(!refused.ok, "{bad:?}");
    }
    assert_eq!(cursors(&admin).await, before);

    // With the pools gone the default set moves; github stays excluded.
    close_pool(app).await;
    close_pool(admin).await;
    wait_for_settled_apply(&harness).await;
    let applied = run(&harness, &["--apply", "--reason", "ticket-1"]).await;
    assert!(applied.ok, "{}", applied.output);
    assert_eq!(applied.report["reason"], "ticket-1");
    assert_eq!(
        names(&applied.report["excluded"]),
        vec!["github", "search-index"]
    );
    let admin = self::admin(&harness).await;
    let after = cursors(&admin).await;
    assert!(after.contains(&("notifications".into(), x2.clone(), s2)));
    assert!(after.contains(&("github".into(), "0".into(), 0)));
    assert_eq!(counts(&admin).await, totals);
    close_pool(admin).await;

    wait_for_settled_apply(&harness).await;
    let github = run(
        &harness,
        &["--consumer", "github", "--apply", "--reason", "ticket-2"],
    )
    .await;
    assert!(github.ok, "{}", github.output);
    let admin = self::admin(&harness).await;
    assert!(cursors(&admin).await.contains(&("github".into(), x2, s2)));
    close_pool(admin).await;
    harness.cleanup().await;
}

#[tokio::test]
async fn apply_rewinds_and_redelivers_exactly_once_then_is_idempotent() {
    let harness = TestDb::bootstrap().await;
    let admin = admin(&harness).await;
    install_delivery_table(&admin, &app_role(&harness)).await;
    let app = app(&harness).await;
    ensure_consumer(&app, "notifications").await.unwrap();

    let mut ids = Vec::new();
    for n in 0..4 {
        ids.push(
            insert_test_event(&app, "test.reset", json!({ "n": n }))
                .await
                .unwrap(),
        );
    }
    // The relay reads only settled events; wait out older transactions
    // elsewhere in the cluster before counting deliveries.
    wait_events_settled(&admin).await;
    let handle = dispatcher(app.clone(), "notifications");
    wait_until("4 notifications deliveries", || {
        let pool = admin.clone();
        Box::pin(async move { delivered(&pool, "notifications", 4).await })
    })
    .await;
    stop(handle).await;

    // Two later events, then a cursor that jumped past them unprocessed (a
    // hand-edited or wrongly advanced position).
    for n in 4..6 {
        ids.push(
            insert_test_event(&app, "test.reset", json!({ "n": n }))
                .await
                .unwrap(),
        );
    }
    let (x6, s6) = event_pos(&admin, ids[5]).await;
    sqlx::query(
        "UPDATE fvoci.outbox_consumers SET last_xact = $1::xid8, last_seq = $2, lease_owner = NULL, lease_until = NULL WHERE consumer = 'notifications'",
    )
    .bind(&x6)
    .bind(s6)
    .execute(&admin)
    .await
    .unwrap();
    let totals = counts(&admin).await;
    let (x4, s4) = event_pos(&admin, ids[3]).await;
    close_pool(app).await;
    close_pool(admin).await;
    wait_for_no_sessions(&harness).await;

    let applied = run(&harness, &["--apply", "--reason", "cursor ahead of marks"]).await;
    assert!(applied.ok, "{}", applied.output);
    assert_eq!(applied.report["mode"], "apply");
    let n = consumer(&applied.report, "notifications");
    assert_eq!(n["direction"], "backward");
    assert_eq!(n["before"]["lastXact"], x6.as_str());
    assert_eq!(n["target"]["lastXact"], x4.as_str());
    assert_eq!(n["target"]["lastSeq"], s4);
    assert_eq!(n["redelivered"], 2);

    // Idempotent: a second apply finds nothing to move.
    wait_for_no_sessions(&harness).await;
    let again = run(&harness, &["--apply", "--reason", "cursor ahead of marks"]).await;
    assert!(again.ok, "{}", again.output);
    assert_eq!(
        consumer(&again.report, "notifications")["direction"],
        "unchanged"
    );

    let admin = self::admin(&harness).await;
    assert_eq!(
        counts(&admin).await,
        totals,
        "no event, mark or failure removed"
    );
    let app = self::app(&harness).await;
    wait_events_settled(&admin).await;
    let handle = dispatcher(app.clone(), "notifications");
    wait_until("6 notifications deliveries", || {
        let pool = admin.clone();
        Box::pin(async move { delivered(&pool, "notifications", 6).await })
    })
    .await;
    // The cursor tables are the owner's; the app role reaches them only
    // through the outbox functions.
    wait_until("notifications cursor at the newest event", || {
        let pool = admin.clone();
        let want = Some((x6.clone(), s6));
        Box::pin(async move {
            let cursor = fetch_cursor(&pool, "notifications")
                .await
                .map_err(|err| err.to_string())?;
            if cursor == want {
                Ok(())
            } else {
                Err(format!(
                    "cursor {cursor:?}, want {want:?}; {}",
                    xmin_state(&pool).await
                ))
            }
        })
    })
    .await;
    stop(handle).await;
    let mut expected = ids.clone();
    expected.sort();
    assert_eq!(deliveries(&app, "notifications").await, expected);
    let failures: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.outbox_failures")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(failures, 0, "no duplicate delivery failed");
    close_pool(app).await;
    close_pool(admin).await;
    harness.cleanup().await;
}

#[tokio::test]
async fn forward_skip_needs_override_and_live_lease_blocks_apply() {
    let harness = TestDb::bootstrap().await;
    let admin = admin(&harness).await;
    let app = app(&harness).await;
    ensure_consumer(&app, "push").await.unwrap();
    let old = insert_test_event(&app, "test.old", json!({}))
        .await
        .unwrap();
    let recent = insert_test_event(&app, "test.recent", json!({}))
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.events SET created_at = now() - interval '40 days' WHERE id = $1")
        .bind(old)
        .execute(&admin)
        .await
        .unwrap();
    mark_processed(&app, "push", recent).await.unwrap();
    assert!(lease_consumer(&app, "push", Uuid::now_v7(), 60)
        .await
        .unwrap());
    let totals = counts(&admin).await;
    let (xr, sr) = event_pos(&admin, recent).await;
    let (xo, so) = event_pos(&admin, old).await;

    let diag = run(&harness, &["--consumer", "push"]).await;
    assert!(diag.ok, "{}", diag.output);
    let push = consumer(&diag.report, "push");
    assert_eq!(push["leaseActive"], true);
    assert_eq!(push["direction"], "forward");
    assert_eq!(push["target"]["lastXact"], xr.as_str());
    assert_eq!(push["skip"]["skippedCount"], 1);
    assert_eq!(push["skip"]["minXact"], xo.as_str());
    assert_eq!(push["skip"]["minSeq"], so);
    assert_eq!(push["skip"]["maxXact"], xo.as_str());
    assert_eq!(push["skip"]["sample"][0]["eventId"], old.to_string());
    assert_eq!(push["skip"]["sample"][0]["verb"], "test.old");

    close_pool(app).await;
    close_pool(admin).await;
    wait_for_no_sessions(&harness).await;
    let leased = run(
        &harness,
        &[
            "--consumer",
            "push",
            "--apply",
            "--reason",
            "r",
            "--override-reason",
            "ack",
        ],
    )
    .await;
    assert!(!leased.ok);
    assert!(leased.output.contains("active lease"), "{}", leased.output);

    let admin = self::admin(&harness).await;
    sqlx::query("UPDATE fvoci.outbox_consumers SET lease_owner = NULL, lease_until = NULL")
        .execute(&admin)
        .await
        .unwrap();
    close_pool(admin).await;
    wait_for_no_sessions(&harness).await;
    let no_override = run(
        &harness,
        &["--consumer", "push", "--apply", "--reason", "r"],
    )
    .await;
    assert!(!no_override.ok);
    assert!(
        no_override.output.contains("forward skip"),
        "{}",
        no_override.output
    );
    let admin = self::admin(&harness).await;
    assert!(cursors(&admin)
        .await
        .contains(&("push".into(), "0".into(), 0)));
    close_pool(admin).await;

    wait_for_settled_apply(&harness).await;
    let overridden = run(
        &harness,
        &[
            "--consumer",
            "push",
            "--apply",
            "--reason",
            "r",
            "--override-reason",
            "ack",
        ],
    )
    .await;
    assert!(overridden.ok, "{}", overridden.output);
    assert_eq!(overridden.report["overrideReason"], "ack");
    let push = consumer(&overridden.report, "push");
    assert_eq!(push["skip"]["skippedCount"], 1);
    assert_eq!(push["direction"], "forward");

    let admin = self::admin(&harness).await;
    assert!(cursors(&admin).await.contains(&("push".into(), xr, sr)));
    assert_eq!(counts(&admin).await, totals, "the skipped event is kept");
    close_pool(admin).await;
    harness.cleanup().await;
}

async fn set_cursor(admin: &PgPool, name: &str, pos: &(String, i64)) {
    sqlx::query(
        "UPDATE fvoci.outbox_consumers SET last_xact = $2::xid8, last_seq = $3 WHERE consumer = $1",
    )
    .bind(name)
    .bind(&pos.0)
    .bind(pos.1)
    .execute(admin)
    .await
    .unwrap();
}

fn pos_of(value: &Value) -> (String, i64) {
    (
        value["lastXact"].as_str().unwrap().to_string(),
        value["lastSeq"].as_i64().unwrap(),
    )
}

/// Review F1: push and webhooks seeded at the tail by 040/027 hold no marks
/// for older events. The default apply must not replay that history to
/// external endpoints; `--ack-external-replay` does and reports it.
#[tokio::test]
async fn tail_seeded_external_consumers_need_ack_to_replay_history() {
    let harness = TestDb::bootstrap().await;
    let admin = admin(&harness).await;
    let app = app(&harness).await;
    for name in ["notifications", "push", "webhooks"] {
        ensure_consumer(&app, name).await.unwrap();
    }
    let mut ids = Vec::new();
    for n in 0..5 {
        ids.push(
            insert_test_event(&app, "test.reset", json!({ "n": n }))
                .await
                .unwrap(),
        );
    }
    let mut pos = Vec::new();
    for id in &ids {
        pos.push(event_pos(&admin, *id).await);
    }
    let origin = ("0".to_string(), 0_i64);
    // Seeded at e3 (the tail when 040/027 ran). webhooks then marked e4 and
    // e5; push marked e4 and lost e5, a post-seed gap it may replay.
    for id in &ids {
        mark_processed(&app, "notifications", *id).await.unwrap();
    }
    mark_processed(&app, "webhooks", ids[3]).await.unwrap();
    mark_processed(&app, "webhooks", ids[4]).await.unwrap();
    mark_processed(&app, "push", ids[3]).await.unwrap();
    set_cursor(&admin, "notifications", &pos[2]).await;
    set_cursor(&admin, "webhooks", &pos[4]).await;
    set_cursor(&admin, "push", &pos[4]).await;
    // e1 dead-lettered for push: the dispatcher passes it, so it is counted apart.
    sqlx::query(
        "INSERT INTO fvoci.outbox_failures (consumer, event_id, attempts, last_error, next_attempt_at, dead_at) VALUES ('push', $1, 5, 'gone', now(), now())",
    )
    .bind(ids[0])
    .execute(&admin)
    .await
    .unwrap();
    let totals = counts(&admin).await;

    let diag = run(&harness, &[]).await;
    assert!(diag.ok, "{}", diag.output);
    assert_eq!(diag.report["ackExternalReplay"], false);
    let n = consumer(&diag.report, "notifications");
    assert_eq!(n["externalEffects"], false);
    assert_eq!(n["direction"], "forward");
    assert_eq!(pos_of(&n["target"]), pos[4]);
    assert!(n.get("externalReplay").is_none(), "{n}");
    let push = consumer(&diag.report, "push");
    assert_eq!(push["externalEffects"], true);
    assert_eq!(push["direction"], "backward");
    assert_eq!(pos_of(&push["target"]), pos[3]);
    assert_eq!(push["redelivered"], 1);
    let replay = &push["externalReplay"];
    assert_eq!(pos_of(&replay["floor"]), pos[2]);
    assert_eq!(pos_of(&replay["target"]), origin);
    assert_eq!(replay["redelivered"], 3);
    assert_eq!(replay["deadLettered"], 1);
    assert_eq!(replay["acknowledged"], false);
    let webhooks = consumer(&diag.report, "webhooks");
    assert_eq!(webhooks["direction"], "unchanged");
    assert_eq!(pos_of(&webhooks["externalReplay"]["floor"]), pos[2]);
    assert_eq!(webhooks["externalReplay"]["redelivered"], 3);

    close_pool(app).await;
    close_pool(admin).await;
    wait_for_settled_apply(&harness).await;
    let applied = run(&harness, &["--apply", "--reason", "routine"]).await;
    assert!(applied.ok, "{}", applied.output);
    let admin = self::admin(&harness).await;
    let after = cursors(&admin).await;
    assert!(after.contains(&("notifications".into(), pos[4].0.clone(), pos[4].1)));
    assert!(after.contains(&("push".into(), pos[3].0.clone(), pos[3].1)));
    assert!(after.contains(&("webhooks".into(), pos[4].0.clone(), pos[4].1)));
    set_cursor(&admin, "push", &pos[4]).await;
    close_pool(admin).await;

    wait_for_settled_apply(&harness).await;
    let acked = run(
        &harness,
        &["--apply", "--reason", "replay", "--ack-external-replay"],
    )
    .await;
    assert!(acked.ok, "{}", acked.output);
    assert_eq!(acked.report["ackExternalReplay"], true);
    for name in ["push", "webhooks"] {
        let c = consumer(&acked.report, name);
        assert_eq!(c["direction"], "backward", "{name}");
        assert_eq!(pos_of(&c["target"]), origin, "{name}");
        assert_eq!(c["externalReplay"]["acknowledged"], true, "{name}");
    }
    assert_eq!(consumer(&acked.report, "push")["redelivered"], 3);
    assert_eq!(consumer(&acked.report, "push")["deadLettered"], 1);
    assert_eq!(consumer(&acked.report, "webhooks")["redelivered"], 3);
    let admin = self::admin(&harness).await;
    let after = cursors(&admin).await;
    assert!(after.contains(&("push".into(), "0".into(), 0)));
    assert!(after.contains(&("webhooks".into(), "0".into(), 0)));
    assert_eq!(counts(&admin).await, totals);
    close_pool(admin).await;
    harness.cleanup().await;
}

/// Review F2: a role that cannot see other roles' sessions is refused, and a
/// forward target at or above snapshot xmin is refused while the older
/// transaction runs (here in another database, which the session count does
/// not see but xmin does).
#[tokio::test]
async fn apply_refuses_blind_owner_and_in_flight_xmin() {
    let harness = TestDb::bootstrap().await;
    let admin = admin(&harness).await;
    let suffix = Uuid::now_v7().simple().to_string();
    // Cluster-level roles: dropped below even when an assertion fails.
    let roles = [
        format!("fvoci_reset_blind_{suffix}"),
        format!("fvoci_reset_noinherit_{suffix}"),
        format!("fvoci_reset_inheritfalse_{suffix}"),
    ];
    let password = Uuid::now_v7().simple().to_string();
    for (role, inherit) in roles.iter().zip(["INHERIT", "NOINHERIT", "INHERIT"]) {
        sqlx::query(&format!(
            "CREATE ROLE {role} LOGIN NOSUPERUSER {inherit} PASSWORD '{password}'"
        ))
        .execute(&admin)
        .await
        .unwrap();
    }
    // Members of pg_read_all_stats without its privileges stay blind.
    sqlx::query(&format!("GRANT pg_read_all_stats TO {}", roles[1]))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!(
        "GRANT pg_read_all_stats TO {} WITH INHERIT FALSE",
        roles[2]
    ))
    .execute(&admin)
    .await
    .unwrap();
    close_pool(admin).await;

    let result = AssertUnwindSafe(blind_and_xmin_case(&harness, &roles, &password))
        .catch_unwind()
        .await;

    let admin = self::admin(&harness).await;
    for role in &roles {
        sqlx::query(&format!("DROP ROLE IF EXISTS {role}"))
            .execute(&admin)
            .await
            .unwrap();
    }
    close_pool(admin).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    harness.cleanup().await;
}

async fn blind_and_xmin_case(harness: &TestDb, roles: &[String], password: &str) {
    let admin = admin(harness).await;
    let app = app(harness).await;
    ensure_consumer(&app, "notifications").await.unwrap();

    let first = insert_test_event(&app, "test.reset", json!({ "n": 0 }))
        .await
        .unwrap();
    mark_processed(&app, "notifications", first).await.unwrap();
    let before = cursors(&admin).await;

    // Sessions blocked from pg_stat_activity: refused even with the app
    // connected, including members that do not inherit pg_read_all_stats.
    for role in roles {
        let mut blind_url = url::Url::parse(&harness.admin_url).unwrap();
        blind_url.set_username(role).unwrap();
        blind_url.set_password(Some(password)).unwrap();
        let blind = run_as(blind_url.as_str(), &["--apply", "--reason", "r"]).await;
        assert!(!blind.ok, "{role}: {}", blind.output);
        assert!(
            blind.output.contains("pg_read_all_stats"),
            "{role}: {}",
            blind.output
        );
        assert_eq!(cursors(&admin).await, before);
    }

    // A transaction with an xid, older than the next event, stays open.
    let mut server = url::Url::parse(&harness.admin_url).unwrap();
    server.set_path("/postgres");
    let mut blocker = sqlx::PgConnection::connect(server.as_str()).await.unwrap();
    sqlx::query("BEGIN").execute(&mut blocker).await.unwrap();
    let _: String = sqlx::query_scalar("SELECT pg_current_xact_id()::text")
        .fetch_one(&mut blocker)
        .await
        .unwrap();
    let second = insert_test_event(&app, "test.reset", json!({ "n": 1 }))
        .await
        .unwrap();
    mark_processed(&app, "notifications", second).await.unwrap();
    let (x2, s2) = event_pos(&admin, second).await;

    close_pool(app).await;
    close_pool(admin).await;
    wait_for_no_sessions(harness).await;
    let in_flight = run(harness, &["--apply", "--reason", "r"]).await;
    assert!(!in_flight.ok);
    assert!(
        in_flight.output.contains("snapshot xmin"),
        "{}",
        in_flight.output
    );

    sqlx::query("ROLLBACK").execute(&mut blocker).await.unwrap();
    blocker.close().await.unwrap();
    wait_for_settled_apply(harness).await;
    let applied = run(harness, &["--apply", "--reason", "r"]).await;
    assert!(applied.ok, "{}", applied.output);
    let admin = self::admin(harness).await;
    assert!(cursors(&admin)
        .await
        .contains(&("notifications".into(), x2, s2)));
    close_pool(admin).await;
}
