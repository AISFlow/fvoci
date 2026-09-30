#![cfg(feature = "db-tests")]

#[allow(dead_code)]
#[path = "support/project_harness.rs"]
mod project_harness;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fvoci_server::db::outbox::{
    advance_cursor, advance_cursor_tx, claim_retries, ensure_consumer, fetch_cursor,
    fetch_event_by_id, fetch_failure_state, insert_test_event, is_outbox_xid_epoch_mismatch,
    is_processed, lease_consumer, mark_processed, read_events, record_failure, release_consumer,
    requeue, OUTBOX_DEFAULT_BATCH, OUTBOX_LEASE_SECS, OUTBOX_MAX_ATTEMPTS,
};
use fvoci_server::db::outbox_recover::{recover_outbox, RecoverOutboxOptions};
use fvoci_server::db::{migrate, pool};
use fvoci_server::outbox::{
    spawn_outbox_dispatcher, DeliveryMode, OutboxConsumer, OutboxDispatcherSettings,
    OutboxProcessError,
};
use rand::RngCore;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

struct TestDb {
    admin_url: String,
    app_url: String,
    db_name: String,
    role_name: String,
}

impl TestDb {
    async fn bootstrap() -> Self {
        let admin_base = std::env::var("TEST_DATABASE_URL")
            .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
            .expect("TEST_DATABASE_URL missing");

        let db_name = format!("fvoci_outbox_{}", Uuid::now_v7().simple());
        let role_name = format!("fvoci_app_{}", db_name.replace('-', "_"));
        let mut password_bytes = [0u8; 24];
        rand::rng().fill_bytes(&mut password_bytes);
        let role_password = hex::encode(password_bytes);
        let server_url = server_db_url(&admin_base);

        let admin_pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&server_url)
            .await
            .expect("connect admin");
        sqlx::query(&format!("CREATE DATABASE \"{}\"", db_name))
            .execute(&admin_pool)
            .await
            .expect("create database");
        admin_pool.close().await;

        let admin_url = join_db_url(&server_url, &db_name);
        migrate::run_migrations(&admin_url).await.expect("migrate");

        let migration_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&admin_url)
            .await
            .expect("connect migration db");
        sqlx::query(&format!(
            "CREATE ROLE \"{}\" LOGIN PASSWORD '{}' NOSUPERUSER NOBYPASSRLS",
            role_name, role_password
        ))
        .execute(&migration_pool)
        .await
        .expect("create role");
        migrate::apply_app_role_grants(&migration_pool, &role_name)
            .await
            .expect("grant");
        migration_pool.close().await;

        let mut app = url::Url::parse(&admin_url).expect("database url");
        app.set_username(&role_name).ok();
        app.set_password(Some(&role_password)).ok();

        Self {
            admin_url,
            app_url: app.to_string(),
            db_name,
            role_name,
        }
    }

    async fn cleanup(self) {
        let server_url = server_db_url(&self.admin_url);
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&server_url)
            .await
            .ok();
        if let Some(pool) = pool {
            let _ = sqlx::query(&format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
                self.db_name
            ))
            .execute(&pool)
            .await;
            let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", self.db_name))
                .execute(&pool)
                .await;
            let _ = sqlx::query(&format!("DROP ROLE IF EXISTS \"{}\"", self.role_name))
                .execute(&pool)
                .await;
            pool.close().await;
        }
    }
}

fn server_db_url(url: &str) -> String {
    let parsed = url::Url::parse(url).expect("database url");
    let mut server = parsed.clone();
    server.set_path("/postgres");
    server.to_string()
}

fn join_db_url(server_url: &str, db_name: &str) -> String {
    let mut url = url::Url::parse(server_url).expect("database url");
    url.set_path(&format!("/{}", db_name));
    url.to_string()
}

async fn install_pin_table(admin: &PgPool) {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS fvoci.outbox_xmin_pin (
            id integer PRIMARY KEY,
            touched timestamptz NOT NULL DEFAULT now()
        )
        "#,
    )
    .execute(admin)
    .await
    .expect("pin table");
    sqlx::query("INSERT INTO fvoci.outbox_xmin_pin (id) VALUES (1) ON CONFLICT (id) DO NOTHING")
        .execute(admin)
        .await
        .expect("pin seed");
}

async fn begin_xmin_pin(admin: &PgPool) -> sqlx::Transaction<'_, sqlx::Postgres> {
    let mut tx = admin.begin().await.expect("pin tx");
    sqlx::query("UPDATE fvoci.outbox_xmin_pin SET touched = now() WHERE id = 1")
        .execute(&mut *tx)
        .await
        .expect("pin write");
    tx
}

async fn install_delivery_table(admin: &PgPool, app_role: &str) {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS fvoci.outbox_test_deliveries (
            consumer text NOT NULL,
            event_id uuid NOT NULL,
            verb text NOT NULL,
            PRIMARY KEY (consumer, event_id)
        )
        "#,
    )
    .execute(admin)
    .await
    .expect("delivery table");
    sqlx::query(&format!(
        "GRANT SELECT, INSERT ON fvoci.outbox_test_deliveries TO \"{}\"",
        app_role.replace('"', "\"\"")
    ))
    .execute(admin)
    .await
    .expect("grant delivery table");
}

struct PgOnlyTestConsumer {
    name: String,
    crash_before_advance: AtomicBool,
}

impl PgOnlyTestConsumer {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            crash_before_advance: AtomicBool::new(false),
        }
    }
}

impl OutboxConsumer for PgOnlyTestConsumer {
    fn name(&self) -> &str {
        &self.name
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::PgOnly
    }

    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        event: &'a fvoci_server::db::outbox::OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            let mut tx = pool.begin().await?;
            // Non-idempotent: a second apply must error, not hide behind ON CONFLICT.
            sqlx::query(
                r#"
                INSERT INTO fvoci.outbox_test_deliveries (consumer, event_id, verb)
                VALUES ($1, $2, $3)
                "#,
            )
            .bind(self.name())
            .bind(event.id)
            .bind(&event.verb)
            .execute(&mut *tx)
            .await?;

            if self.crash_before_advance.load(Ordering::SeqCst) {
                tx.rollback().await?;
                return Err(OutboxProcessError::Delivery("simulated crash".into()));
            }

            if !advance_cursor_tx(&mut tx, self.name(), lease_owner, &event.xact, event.seq).await?
            {
                tx.rollback().await?;
                return Err(OutboxProcessError::Delivery(
                    "advance rejected in pg-only tx".into(),
                ));
            }
            tx.commit().await?;
            Ok(())
        })
    }
}

struct FailingConsumer {
    name: String,
    fail_until: AtomicI32,
}

impl FailingConsumer {
    fn new(name: &str, fail_until: i32) -> Self {
        Self {
            name: name.to_string(),
            fail_until: AtomicI32::new(fail_until),
        }
    }
}

impl OutboxConsumer for FailingConsumer {
    fn name(&self) -> &str {
        &self.name
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::External
    }

    fn deliver<'a>(
        &'a self,
        _pool: &'a PgPool,
        _lease_owner: Uuid,
        _event: &'a fvoci_server::db::outbox::OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            let remaining = self.fail_until.fetch_sub(1, Ordering::SeqCst);
            if remaining > 0 {
                Err(OutboxProcessError::Delivery(format!(
                    "injected failure ({remaining} remaining)"
                )))
            } else {
                Ok(())
            }
        })
    }
}

async fn delivery_count(pool: &PgPool, consumer: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM fvoci.outbox_test_deliveries WHERE consumer = $1")
        .bind(consumer)
        .fetch_one(pool)
        .await
        .expect("delivery count")
}

async fn wait_until<F>(timeout: Duration, mut predicate: F)
where
    F: FnMut() -> Pin<Box<dyn Future<Output = bool> + Send>>,
{
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if predicate().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("condition not met within {:?}", timeout);
}

async fn wait_until_readable(app: &PgPool, consumer: &str, event_id: Uuid) {
    let consumer = consumer.to_string();
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        let consumer = consumer.clone();
        Box::pin(async move {
            read_events(&pool, &consumer, 100)
                .await
                .ok()
                .is_some_and(|rows| rows.iter().any(|event| event.id == event_id))
        })
    })
    .await;
}

const DISPATCHER_WAIT: Duration = Duration::from_secs(15);

async fn wait_for_client_backends_gone(admin_url: &str, db_name: &str) {
    let server = server_db_url(admin_url);
    let observer = PgPoolOptions::new()
        .max_connections(1)
        .connect(&server)
        .await
        .expect("observer");
    let name = db_name.to_string();
    wait_until(DISPATCHER_WAIT, || {
        let pool = observer.clone();
        let name = name.clone();
        Box::pin(async move {
            let n: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE datname = $1 AND backend_type = 'client backend'",
            )
            .bind(name)
            .fetch_one(&pool)
            .await
            .unwrap_or(1);
            n == 0
        })
    })
    .await;
    observer.close().await;
}

fn run_dispatcher(
    pool: PgPool,
    consumer: Arc<dyn OutboxConsumer>,
    poll_ms: u64,
) -> fvoci_server::outbox::OutboxDispatcherHandle {
    spawn_outbox_dispatcher(
        OutboxDispatcherSettings {
            poll_interval: Duration::from_millis(poll_ms),
            lease_ttl: Duration::from_secs(2),
            batch_limit: 50,
            failure_backoff: Duration::from_millis(50),
        },
        pool,
        vec![consumer],
    )
    .expect("dispatcher requires at least one consumer")
}

#[tokio::test]
async fn delivers_events_in_xact_seq_order() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    install_delivery_table(&admin, &harness.role_name).await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");

    for idx in 0..5 {
        insert_test_event(&app, "test.ordered", json!({ "n": idx }))
            .await
            .expect("insert");
    }
    let consumer = Arc::new(PgOnlyTestConsumer::new("ordered"));
    let dispatcher = run_dispatcher(app.clone(), consumer, 20);
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        Box::pin(async move { delivery_count(&pool, "ordered").await == 5 })
    })
    .await;

    let rows = sqlx::query(
        r#"
        SELECT d.verb, (e.payload ->> 'n')::int AS n
        FROM fvoci.outbox_test_deliveries d
        INNER JOIN fvoci.events e ON e.id = d.event_id
        WHERE d.consumer = 'ordered'
        ORDER BY e.xact, e.seq
        "#,
    )
    .fetch_all(&admin)
    .await
    .expect("rows");
    assert_eq!(rows.len(), 5);
    for (idx, row) in rows.iter().enumerate() {
        assert_eq!(row.get::<i32, _>("n"), idx as i32);
    }

    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn inverted_commit_order_loses_nothing() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    install_delivery_table(&admin, &harness.role_name).await;
    install_pin_table(&admin).await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");

    let mut tx1 = admin.begin().await.expect("tx1");
    let id1 = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO fvoci.events (id, verb, payload, channel) VALUES ($1, 'first', '{}', 'system')",
    )
    .bind(id1)
    .execute(&mut *tx1)
    .await
    .expect("insert1");

    let mut tx2 = admin.begin().await.expect("tx2");
    let id2 = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO fvoci.events (id, verb, payload, channel) VALUES ($1, 'second', '{}', 'system')",
    )
    .bind(id2)
    .execute(&mut *tx2)
    .await
    .expect("insert2");

    tx2.commit().await.expect("commit2");

    let consumer = Arc::new(PgOnlyTestConsumer::new("invert"));
    let dispatcher = run_dispatcher(app.clone(), consumer, 20);
    wait_until(DISPATCHER_WAIT, || {
        let pool = admin.clone();
        Box::pin(async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT lease_owner IS NOT NULL FROM fvoci.outbox_consumers WHERE consumer = 'invert'",
            )
            .fetch_optional(&pool)
            .await
            .expect("lease")
            .unwrap_or(false)
        })
    })
    .await;
    assert_eq!(delivery_count(&app, "invert").await, 0);

    tx1.commit().await.expect("commit1");

    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        Box::pin(async move { delivery_count(&pool, "invert").await == 2 })
    })
    .await;

    let order = sqlx::query(
        r#"
        SELECT e.verb
        FROM fvoci.outbox_test_deliveries d
        INNER JOIN fvoci.events e ON e.id = d.event_id
        WHERE d.consumer = 'invert'
        ORDER BY e.xact, e.seq
        "#,
    )
    .fetch_all(&admin)
    .await
    .expect("order");
    assert_eq!(order.len(), 2);
    assert_eq!(order[0].get::<String, _>("verb"), "first");
    assert_eq!(order[1].get::<String, _>("verb"), "second");

    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn long_open_transaction_stalls_without_skipping() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    install_delivery_table(&admin, &harness.role_name).await;
    install_pin_table(&admin).await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");

    insert_test_event(&app, "baseline", json!({}))
        .await
        .expect("baseline");

    let consumer = Arc::new(PgOnlyTestConsumer::new("stall"));
    let dispatcher = run_dispatcher(app.clone(), consumer, 20);
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        Box::pin(async move { delivery_count(&pool, "stall").await == 1 })
    })
    .await;

    let holder = begin_xmin_pin(&admin).await;

    insert_test_event(&app, "stalled", json!({}))
        .await
        .expect("stalled");
    let marker: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&admin)
        .await
        .expect("marker");

    wait_until(DISPATCHER_WAIT, || {
        let pool = admin.clone();
        Box::pin(async move {
            let updated: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
                "SELECT updated_at FROM fvoci.outbox_consumers WHERE consumer = 'stall'",
            )
            .fetch_optional(&pool)
            .await
            .expect("updated_at");
            updated.is_some_and(|ts| ts > marker)
        })
    })
    .await;
    assert_eq!(delivery_count(&app, "stall").await, 1);

    holder.commit().await.expect("release holder");

    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        Box::pin(async move { delivery_count(&pool, "stall").await == 2 })
    })
    .await;

    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn lease_steal_after_expiry() {
    let harness = TestDb::bootstrap().await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let consumer = "lease-test";
    ensure_consumer(&app, consumer).await.expect("ensure");

    let owner1 = Uuid::now_v7();
    assert!(lease_consumer(&app, consumer, owner1, 1)
        .await
        .expect("lease1"));

    let owner2 = Uuid::now_v7();
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        Box::pin(async move {
            lease_consumer(&pool, consumer, owner2, 30)
                .await
                .expect("lease2 poll")
        })
    })
    .await;
    assert!(!lease_consumer(&app, consumer, owner1, 30)
        .await
        .expect("old owner"));

    let _ = release_consumer(&app, consumer, owner2).await;
    app.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn advance_rejected_without_lease() {
    let harness = TestDb::bootstrap().await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let consumer = "no-lease";
    ensure_consumer(&app, consumer).await.expect("ensure");
    let event_id = insert_test_event(&app, "x", json!({}))
        .await
        .expect("event");
    let event = fvoci_server::db::outbox::fetch_event_by_id(&app, event_id)
        .await
        .expect("fetch")
        .expect("row");
    wait_until_readable(&app, consumer, event_id).await;

    let stranger = Uuid::now_v7();
    assert!(
        !advance_cursor(&app, consumer, stranger, &event.xact, event.seq)
            .await
            .expect("advance")
    );

    let owner = Uuid::now_v7();
    assert!(lease_consumer(&app, consumer, owner, 30)
        .await
        .expect("lease"));
    assert!(
        advance_cursor(&app, consumer, owner, &event.xact, event.seq)
            .await
            .expect("advance leased")
    );

    app.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn pg_only_crash_between_effect_and_advance_rolls_back() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    install_delivery_table(&admin, &harness.role_name).await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");

    let event_id = insert_test_event(&app, "once", json!({}))
        .await
        .expect("event");
    let consumer = Arc::new(PgOnlyTestConsumer::new("exactly-once"));
    consumer.crash_before_advance.store(true, Ordering::SeqCst);
    let dispatcher = run_dispatcher(app.clone(), consumer.clone(), 20);

    wait_until(DISPATCHER_WAIT, || {
        let pool = admin.clone();
        Box::pin(async move {
            let attempts = sqlx::query_scalar::<_, i32>(
                "SELECT attempts FROM fvoci.outbox_failures WHERE consumer = 'exactly-once' AND event_id = $1",
            )
            .bind(event_id)
            .fetch_optional(&pool)
            .await
            .expect("failures");
            attempts.unwrap_or(0) > 0
        })
    })
    .await;

    assert_eq!(delivery_count(&app, "exactly-once").await, 0);

    consumer.crash_before_advance.store(false, Ordering::SeqCst);
    dispatcher.wake.notify_one();

    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        Box::pin(async move { delivery_count(&pool, "exactly-once").await == 1 })
    })
    .await;

    // The dispatcher clears the failure row right after the delivery commits (a
    // separate statement), so wait for it (bounded, read-only) rather than
    // reading it at the instant the effect becomes visible.
    wait_until(DISPATCHER_WAIT, || {
        let pool = admin.clone();
        Box::pin(async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM fvoci.outbox_failures WHERE consumer = 'exactly-once' AND event_id = $1",
            )
            .bind(event_id)
            .fetch_one(&pool)
            .await
            .expect("failures")
                == 0
        })
    })
    .await;
    assert_eq!(delivery_count(&app, "exactly-once").await, 1);

    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn dead_letter_after_max_failures_and_sweep_retry() {
    let harness = TestDb::bootstrap().await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let event_id = insert_test_event(&app, "poison", json!({}))
        .await
        .expect("event");
    let event = fvoci_server::db::outbox::fetch_event_by_id(&app, event_id)
        .await
        .expect("fetch")
        .expect("row");

    let consumer_name = "dead-letter";
    let owner = Uuid::now_v7();
    assert!(lease_consumer(&app, consumer_name, owner, 30)
        .await
        .expect("lease"));
    wait_until_readable(&app, consumer_name, event_id).await;

    for _ in 0..OUTBOX_MAX_ATTEMPTS {
        let attempts = record_failure(
            &app,
            consumer_name,
            owner,
            event_id,
            "boom",
            1,
            OUTBOX_MAX_ATTEMPTS,
        )
        .await
        .expect("record");
        if attempts >= OUTBOX_MAX_ATTEMPTS {
            assert!(
                advance_cursor(&app, consumer_name, owner, &event.xact, event.seq)
                    .await
                    .expect("advance")
            );
        }
    }

    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let state = fetch_failure_state(&app, consumer_name, event_id)
        .await
        .expect("state")
        .expect("failure row");
    assert_eq!(state.attempts, OUTBOX_MAX_ATTEMPTS);
    assert!(state.dead_at.is_some());

    let cursor = fetch_cursor(&admin, consumer_name).await.expect("cursor");
    assert_eq!(cursor, Some((event.xact.clone(), event.seq)));

    sqlx::query(
        "UPDATE fvoci.outbox_failures SET next_attempt_at = now() - interval '1 second' WHERE consumer = $1 AND event_id = $2",
    )
    .bind(consumer_name)
    .bind(event_id)
    .execute(&admin)
    .await
    .expect("ready retry");

    assert!(release_consumer(&app, consumer_name, owner)
        .await
        .expect("release manual lease"));

    let claimed = claim_retries(&app, consumer_name, 50).await.expect("claim");
    assert!(claimed.is_empty(), "dead letters must not be swept");

    let failing = Arc::new(FailingConsumer::new(consumer_name, 0));
    let dispatcher = run_dispatcher(app.clone(), failing, 20);
    let marker: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&admin)
        .await
        .expect("marker");
    wait_until(DISPATCHER_WAIT, || {
        let pool = admin.clone();
        Box::pin(async move {
            let updated: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
                "SELECT updated_at FROM fvoci.outbox_consumers WHERE consumer = 'dead-letter'",
            )
            .fetch_optional(&pool)
            .await
            .expect("updated");
            updated.is_some_and(|ts| ts > marker)
        })
    })
    .await;

    let still_dead = fetch_failure_state(&app, consumer_name, event_id)
        .await
        .expect("state after sweep")
        .expect("dead row remains");
    assert_eq!(still_dead.attempts, OUTBOX_MAX_ATTEMPTS);
    assert!(still_dead.dead_at.is_some());

    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");

    assert!(requeue(&app, consumer_name, event_id)
        .await
        .expect("operator requeue"));
    let requeued = fetch_failure_state(&app, consumer_name, event_id)
        .await
        .expect("requeued")
        .expect("row");
    assert!(requeued.dead_at.is_none());
    assert_eq!(requeued.attempts, 1);

    admin.close().await;
    app.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn dispatcher_shutdown_drains_within_deadline() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    install_delivery_table(&admin, &harness.role_name).await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");

    let consumer = Arc::new(PgOnlyTestConsumer::new("shutdown"));
    let dispatcher = run_dispatcher(app.clone(), consumer, 100);
    let started = std::time::Instant::now();
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
    assert!(started.elapsed() < Duration::from_secs(2));

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

struct SelectiveFailPgOnly {
    name: String,
    fail_id: Mutex<Option<Uuid>>,
    fail_forever: AtomicBool,
}

impl OutboxConsumer for SelectiveFailPgOnly {
    fn name(&self) -> &str {
        &self.name
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::PgOnly
    }

    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        event: &'a fvoci_server::db::outbox::OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            {
                let mut guard = self.fail_id.lock().expect("fail_id");
                if *guard == Some(event.id) {
                    if !self.fail_forever.load(Ordering::SeqCst) {
                        *guard = None;
                    }
                    return Err(OutboxProcessError::Delivery("fail once".into()));
                }
            }
            let mut tx = pool.begin().await?;
            sqlx::query(
                r#"
                INSERT INTO fvoci.outbox_test_deliveries (consumer, event_id, verb)
                VALUES ($1, $2, $3)
                "#,
            )
            .bind(self.name())
            .bind(event.id)
            .bind(&event.verb)
            .execute(&mut *tx)
            .await?;
            if !advance_cursor_tx(&mut tx, self.name(), lease_owner, &event.xact, event.seq).await?
            {
                tx.rollback().await?;
                return Err(OutboxProcessError::Delivery(
                    "advance rejected in pg-only tx".into(),
                ));
            }
            tx.commit().await?;
            Ok(())
        })
    }
}

#[tokio::test]
async fn pg_only_head_of_line_failure_does_not_skip_later_event() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    install_delivery_table(&admin, &harness.role_name).await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let a = insert_test_event(&app, "A", json!({})).await.expect("A");
    let b = insert_test_event(&app, "B", json!({})).await.expect("B");
    let consumer = Arc::new(SelectiveFailPgOnly {
        name: "r1".into(),
        fail_id: Mutex::new(Some(a)),
        fail_forever: AtomicBool::new(false),
    });
    let dispatcher = run_dispatcher(app.clone(), consumer, 20);
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        Box::pin(async move { delivery_count(&pool, "r1").await == 2 })
    })
    .await;

    let delivered_a: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM fvoci.outbox_test_deliveries WHERE consumer = 'r1' AND event_id = $1",
    )
    .bind(a)
    .fetch_one(&admin)
    .await
    .expect("delivered a");
    let delivered_b: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM fvoci.outbox_test_deliveries WHERE consumer = 'r1' AND event_id = $1",
    )
    .bind(b)
    .fetch_one(&admin)
    .await
    .expect("delivered b");
    assert_eq!(delivered_a, 1);
    assert_eq!(delivered_b, 1);
    assert!(fetch_failure_state(&app, "r1", a)
        .await
        .expect("failure")
        .is_none());

    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn poison_event_dead_letters_and_does_not_retry() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let id = insert_test_event(&app, "poison", json!({}))
        .await
        .expect("ev");
    let failing = Arc::new(FailingConsumer::new("r2", i32::MAX));
    let dispatcher = run_dispatcher(app.clone(), failing, 20);

    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        Box::pin(async move {
            fetch_failure_state(&pool, "r2", id)
                .await
                .expect("state")
                .is_some_and(|row| row.dead_at.is_some())
        })
    })
    .await;

    let dead = fetch_failure_state(&app, "r2", id)
        .await
        .expect("dead")
        .expect("row");
    assert_eq!(dead.attempts, OUTBOX_MAX_ATTEMPTS);
    let marker: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&admin)
        .await
        .expect("marker");
    wait_until(DISPATCHER_WAIT, || {
        let pool = admin.clone();
        Box::pin(async move {
            let updated: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
                "SELECT updated_at FROM fvoci.outbox_consumers WHERE consumer = 'r2'",
            )
            .fetch_optional(&pool)
            .await
            .expect("updated");
            updated.is_some_and(|ts| ts > marker)
        })
    })
    .await;
    let later = fetch_failure_state(&app, "r2", id)
        .await
        .expect("later")
        .expect("row");
    assert_eq!(later.attempts, OUTBOX_MAX_ATTEMPTS);
    assert!(later.dead_at.is_some());

    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn advance_rejects_unknown_event_and_future_xmin() {
    let harness = TestDb::bootstrap().await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let owner = Uuid::now_v7();
    assert!(lease_consumer(&app, "search", owner, 30)
        .await
        .expect("lease"));
    let skipped = advance_cursor(&app, "search", owner, "9223372036854775000", 0)
        .await
        .expect("adv");
    assert!(!skipped);

    let later = insert_test_event(&app, "after", json!({}))
        .await
        .expect("ev");
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        Box::pin(async move {
            read_events(&pool, "search", 100)
                .await
                .ok()
                .is_some_and(|rows| rows.len() == 1)
        })
    })
    .await;
    let rows = read_events(&app, "search", 100).await.expect("read");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, later);

    let unknown = read_events(&app, "anything", 100).await.expect("unknown");
    assert!(unknown.is_empty());
    ensure_consumer(&app, "anything").await.expect("ensure");
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        Box::pin(async move {
            read_events(&pool, "anything", 100)
                .await
                .ok()
                .is_some_and(|rows| rows.len() == 1)
        })
    })
    .await;
    let after_ensure = read_events(&app, "anything", 100)
        .await
        .expect("after ensure");
    assert_eq!(after_ensure.len(), 1);

    let err = ensure_consumer(&app, "BadName").await.expect_err("invalid");
    assert!(
        err.to_string().contains("invalid outbox consumer name")
            || err.to_string().to_lowercase().contains("invalid")
    );

    let _ = release_consumer(&app, "search", owner).await;
    app.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn read_sees_events_when_function_owner_is_subject_to_rls() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let _ = insert_test_event(&app, "x", json!({})).await.expect("ev");
    ensure_consumer(&app, "r4").await.unwrap();
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        Box::pin(async move {
            read_events(&pool, "r4", 100)
                .await
                .ok()
                .is_some_and(|rows| rows.len() == 1)
        })
    })
    .await;
    let before = read_events(&app, "r4", 100).await.unwrap().len();
    assert_eq!(before, 1);

    let owner = format!("{}_own", harness.role_name);
    for q in [
        format!("CREATE ROLE \"{owner}\" NOLOGIN NOSUPERUSER NOBYPASSRLS"),
        format!("GRANT USAGE ON SCHEMA fvoci TO \"{owner}\""),
        format!("ALTER TABLE fvoci.events OWNER TO \"{owner}\""),
        format!("GRANT SELECT ON fvoci.outbox_consumers TO \"{owner}\""),
        format!(
            "GRANT EXECUTE ON FUNCTION public.app_tenant_id(), public.app_system_ctx_on() TO \"{owner}\""
        ),
        format!("ALTER FUNCTION fvoci.app_outbox_read(text, integer) OWNER TO \"{owner}\""),
    ] {
        sqlx::query(&q).execute(&admin).await.expect(&q);
    }
    let after = read_events(&app, "r4", 100)
        .await
        .map(|r| r.len())
        .expect("read under non-bypass owner");
    assert_eq!(after, 1);

    for q in [
        "ALTER TABLE fvoci.events OWNER TO CURRENT_USER".to_string(),
        "ALTER FUNCTION fvoci.app_outbox_read(text, integer) OWNER TO CURRENT_USER".to_string(),
        format!(
            "REVOKE EXECUTE ON FUNCTION public.app_tenant_id(), public.app_system_ctx_on() FROM \"{owner}\""
        ),
        format!("REVOKE ALL ON fvoci.outbox_consumers FROM \"{owner}\""),
        format!("REVOKE ALL ON SCHEMA fvoci FROM \"{owner}\""),
        format!("DROP ROLE \"{owner}\""),
    ] {
        sqlx::query(&q).execute(&admin).await.expect(&q);
    }
    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn spawn_without_consumers_is_idle() {
    let harness = TestDb::bootstrap().await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    assert!(
        spawn_outbox_dispatcher(OutboxDispatcherSettings::default(), app.clone(), Vec::new(),)
            .is_none()
    );
    app.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn pg_only_requeue_applies_after_dead_letter() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    install_delivery_table(&admin, &harness.role_name).await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let a = insert_test_event(&app, "A", json!({})).await.expect("A");
    let b = insert_test_event(&app, "B", json!({})).await.expect("B");
    let consumer = Arc::new(SelectiveFailPgOnly {
        name: "r5".into(),
        fail_id: Mutex::new(Some(a)),
        fail_forever: AtomicBool::new(true),
    });
    let dispatcher = run_dispatcher(app.clone(), consumer.clone(), 20);
    wait_until(DISPATCHER_WAIT, || {
        let pool = admin.clone();
        Box::pin(async move { delivery_count(&pool, "r5").await == 1 })
    })
    .await;
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        Box::pin(async move {
            fetch_failure_state(&pool, "r5", a)
                .await
                .expect("state")
                .is_some_and(|row| row.dead_at.is_some())
        })
    })
    .await;

    let delivered_b: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM fvoci.outbox_test_deliveries WHERE event_id = $1")
            .bind(b)
            .fetch_one(&admin)
            .await
            .expect("delivered b");
    assert_eq!(delivered_b, 1);

    consumer.fail_forever.store(false, Ordering::SeqCst);
    *consumer.fail_id.lock().expect("fail_id") = None;
    assert!(requeue(&app, "r5", a).await.expect("requeue"));
    let requeued = fetch_failure_state(&app, "r5", a)
        .await
        .expect("requeued")
        .expect("row");
    assert!(requeued.dead_at.is_none());
    assert_eq!(requeued.attempts, 1);

    wait_until(DISPATCHER_WAIT, || {
        let pool = admin.clone();
        Box::pin(async move { delivery_count(&pool, "r5").await == 2 })
    })
    .await;
    let delivered_a: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM fvoci.outbox_test_deliveries WHERE consumer = 'r5' AND event_id = $1",
    )
    .bind(a)
    .fetch_one(&admin)
    .await
    .expect("delivered a");
    assert_eq!(delivered_a, 1);
    assert!(fetch_failure_state(&app, "r5", a)
        .await
        .expect("cleared")
        .is_none());

    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn pg_only_already_applied_failure_is_idempotent() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    install_delivery_table(&admin, &harness.role_name).await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let a = insert_test_event(&app, "A", json!({})).await.expect("A");
    let consumer = Arc::new(PgOnlyTestConsumer::new("r6"));
    let dispatcher = run_dispatcher(app.clone(), consumer, 20);
    wait_until(DISPATCHER_WAIT, || {
        let pool = admin.clone();
        Box::pin(async move { delivery_count(&pool, "r6").await == 1 })
    })
    .await;

    let stale_owner = Uuid::now_v7();
    let attempts = record_failure(
        &app,
        "r6",
        stale_owner,
        a,
        "advance rejected in pg-only tx",
        50,
        OUTBOX_MAX_ATTEMPTS,
    )
    .await
    .expect("stale failure refused");
    assert_eq!(attempts, 0);
    assert!(
        fetch_failure_state(&app, "r6", a)
            .await
            .expect("state")
            .is_none(),
        "record_failure must not insert a row at or below the cursor"
    );

    let b = insert_test_event(&app, "B", json!({})).await.expect("B");
    wait_until(DISPATCHER_WAIT, || {
        let pool = admin.clone();
        Box::pin(async move { delivery_count(&pool, "r6").await == 2 })
    })
    .await;
    let delivered_a: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM fvoci.outbox_test_deliveries WHERE consumer = 'r6' AND event_id = $1",
    )
    .bind(a)
    .fetch_one(&admin)
    .await
    .expect("delivered a");
    assert_eq!(delivered_a, 1);
    let delivered_b: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM fvoci.outbox_test_deliveries WHERE consumer = 'r6' AND event_id = $1",
    )
    .bind(b)
    .fetch_one(&admin)
    .await
    .expect("delivered b");
    assert_eq!(delivered_b, 1);
    assert!(fetch_failure_state(&app, "r6", a)
        .await
        .expect("gone")
        .is_none());

    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn cursor_waiting_on_an_older_running_transaction_is_not_an_epoch_mismatch() {
    // After --recover-outbox the cursor sits at the recovery xid; any transaction
    // older than it that is still open (in any database) keeps xmin below it.
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    ensure_consumer(&app, "search-index").await.expect("ensure");

    let mut older = admin.begin().await.expect("older tx");
    sqlx::query("SELECT pg_current_xact_id()")
        .execute(&mut *older)
        .await
        .expect("assign older xid");
    sqlx::query(
        "UPDATE fvoci.outbox_consumers SET last_xact = pg_current_xact_id(), last_seq = 0 \
         WHERE consumer = 'search-index'",
    )
    .execute(&admin)
    .await
    .expect("cursor at a newer committed xid");

    assert!(read_events(&app, "search-index", 10)
        .await
        .expect("read while waiting")
        .is_empty());

    older.rollback().await.expect("rollback older");
    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn xid_epoch_mismatch_refuses_advance_and_recover_rebases() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    install_delivery_table(&admin, &harness.role_name).await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let old = insert_test_event(&app, "old", json!({}))
        .await
        .expect("old");
    ensure_consumer(&app, "search-index").await.expect("ensure");
    wait_until_readable(&app, "search-index", old).await;
    let owner = Uuid::now_v7();
    assert!(lease_consumer(&app, "search-index", owner, 30)
        .await
        .expect("lease"));
    let event = read_events(&app, "search-index", 1)
        .await
        .expect("read")
        .into_iter()
        .next()
        .expect("old event");
    assert_eq!(event.id, old);
    assert!(
        advance_cursor(&app, "search-index", owner, &event.xact, event.seq)
            .await
            .expect("advance old")
    );
    assert!(release_consumer(&app, "search-index", owner)
        .await
        .expect("release"));

    sqlx::query("UPDATE fvoci.events SET xact = '100000000000'::xid8")
        .execute(&admin)
        .await
        .expect("stale event xact");
    sqlx::query("UPDATE fvoci.outbox_consumers SET last_xact = '100000000000'::xid8, last_seq = 1")
        .execute(&admin)
        .await
        .expect("stale cursor");

    let read_err = read_events(&app, "search-index", 10)
        .await
        .expect_err("read must fail closed");
    assert!(is_outbox_xid_epoch_mismatch(&read_err), "{read_err}");
    assert!(lease_consumer(&app, "search-index", owner, 30)
        .await
        .expect("lease after mismatch"));
    assert!(
        !advance_cursor(&app, "search-index", owner, "100000000000", 1)
            .await
            .expect("advance refused")
    );
    assert!(release_consumer(&app, "search-index", owner)
        .await
        .expect("release after mismatch"));

    let later = insert_test_event(&app, "later", json!({}))
        .await
        .expect("later");
    assert_eq!(delivery_count(&admin, "search-index").await, 0);

    let bounds: (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT now() - interval '2 hours', now()")
            .fetch_one(&admin)
            .await
            .expect("recovery bounds");
    let since = bounds
        .0
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    let snapshot_at = bounds
        .1
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);

    project_harness::close_pool(app).await;
    project_harness::close_pool(admin).await;
    wait_for_client_backends_gone(&harness.admin_url, &harness.db_name).await;
    let report = recover_outbox(
        &harness.admin_url,
        RecoverOutboxOptions {
            since: since.clone(),
            snapshot_at: snapshot_at.clone(),
            apply: true,
            reason: Some("test logical restore rebase".into()),
            acknowledge_external_replay: true,
        },
    )
    .await
    .expect("recover");
    assert!(report.applied);
    assert!(report.eligible >= 2, "{report:?}");
    assert_eq!(report.consumers_rebased, 1);

    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin after recover");
    let app = pool::connect_app(&harness.app_url)
        .await
        .expect("app after recover");
    // Rewritten events carry the recovery xid; they become readable once every
    // transaction older than it has ended (xids are cluster-wide, so parallel
    // tests' transactions count). The read itself must never report a mismatch.
    wait_until_readable(&app, "search-index", old).await;
    let visible = read_events(&app, "search-index", 100)
        .await
        .expect("read after recover");
    let ids: Vec<Uuid> = visible.iter().map(|event| event.id).collect();
    assert!(ids.contains(&old), "retained window must replay {ids:?}");
    assert!(
        ids.contains(&later),
        "new cluster event must be visible {ids:?}"
    );

    let consumer = Arc::new(PgOnlyTestConsumer::new("search-index"));
    let dispatcher = run_dispatcher(app.clone(), consumer, 20);
    wait_until(DISPATCHER_WAIT, || {
        let pool = admin.clone();
        Box::pin(async move { delivery_count(&pool, "search-index").await >= 2 })
    })
    .await;
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join after recover");

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

struct CountingPgOnly {
    name: String,
}

impl OutboxConsumer for CountingPgOnly {
    fn name(&self) -> &str {
        &self.name
    }
    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::PgOnly
    }
    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        event: &'a fvoci_server::db::outbox::OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            let mut tx = pool.begin().await?;
            sqlx::query("INSERT INTO fvoci.r8_effects (event_id) VALUES ($1)")
                .bind(event.id)
                .execute(&mut *tx)
                .await?;
            if !advance_cursor_tx(&mut tx, self.name(), lease_owner, &event.xact, event.seq).await?
            {
                tx.rollback().await?;
                return Err(OutboxProcessError::Delivery(
                    "advance rejected in pg-only tx".into(),
                ));
            }
            tx.commit().await?;
            Ok(())
        })
    }
}

#[tokio::test]
async fn r8_stale_owner_failure_row_does_not_double_apply_pg_only_effect() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    sqlx::query("CREATE TABLE fvoci.r8_effects (event_id uuid NOT NULL)")
        .execute(&admin)
        .await
        .expect("effects table");
    sqlx::query(&format!(
        "GRANT SELECT, INSERT ON fvoci.r8_effects TO \"{}\"",
        harness.role_name.replace('"', "\"\"")
    ))
    .execute(&admin)
    .await
    .expect("grant effects");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let e = insert_test_event(&app, "E", json!({})).await.expect("E");
    ensure_consumer(&app, "r8").await.expect("ensure");
    wait_until_readable(&app, "r8", e).await;
    let consumer = Arc::new(CountingPgOnly { name: "r8".into() });

    let a = Uuid::now_v7();
    assert!(lease_consumer(&app, "r8", a, 1).await.expect("lease a"));
    let event = read_events(&app, "r8", 10)
        .await
        .expect("read")
        .into_iter()
        .next()
        .expect("E");
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let b = Uuid::now_v7();
    assert!(lease_consumer(&app, "r8", b, 30).await.expect("lease b"));
    consumer
        .deliver(&app, b, &event)
        .await
        .expect("B applies E");
    assert!(consumer.deliver(&app, a, &event).await.is_err());
    let attempts = record_failure(
        &app,
        "r8",
        a,
        event.id,
        "advance rejected in pg-only tx",
        50,
        OUTBOX_MAX_ATTEMPTS,
    )
    .await
    .expect("stale failure");
    assert_eq!(attempts, 0);
    assert!(fetch_failure_state(&app, "r8", e)
        .await
        .expect("no stale row")
        .is_none());
    assert!(release_consumer(&app, "r8", b).await.expect("release b"));

    let sentinel = insert_test_event(&app, "sentinel", json!({}))
        .await
        .expect("sentinel");
    let dispatcher = run_dispatcher(app.clone(), consumer.clone(), 20);
    wait_until(DISPATCHER_WAIT, || {
        let pool = admin.clone();
        let sentinel_id = sentinel;
        Box::pin(async move {
            let n: i64 =
                sqlx::query_scalar("SELECT count(*) FROM fvoci.r8_effects WHERE event_id = $1")
                    .bind(sentinel_id)
                    .fetch_one(&pool)
                    .await
                    .unwrap_or(0);
            n == 1
        })
    })
    .await;
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
    let applied_e: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.r8_effects WHERE event_id = $1")
            .bind(e)
            .fetch_one(&admin)
            .await
            .expect("count E");
    assert_eq!(applied_e, 1, "PgOnly effect applied {applied_e} times");
    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn r9_epoch_guard_has_no_false_positive_under_concurrent_writes() {
    let harness = TestDb::bootstrap().await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    ensure_consumer(&app, "r9").await.expect("ensure");
    let stop = Arc::new(AtomicBool::new(false));
    let mut writers = Vec::new();
    for _ in 0..6 {
        let pool = app.clone();
        let stop = stop.clone();
        writers.push(tokio::spawn(async move {
            while !stop.load(Ordering::SeqCst) {
                let _ = insert_test_event(&pool, "w", json!({})).await;
            }
        }));
    }
    let mut mismatch = 0usize;
    let mut read_err = 0usize;
    const ITERATIONS: usize = 256;
    for _ in 0..ITERATIONS {
        match read_events(&app, "r9", 1).await {
            Ok(_) => {}
            Err(err) if is_outbox_xid_epoch_mismatch(&err) => mismatch += 1,
            Err(_) => read_err += 1,
        }
    }
    stop.store(true, Ordering::SeqCst);
    for writer in writers {
        let _ = writer.await;
    }
    app.close().await;
    harness.cleanup().await;
    assert_eq!(
        (mismatch, read_err),
        (0, 0),
        "R9 iterations={ITERATIONS} xid_mismatch_true={mismatch} read_errors={read_err}"
    );
}

/// R10: owner A applies a requeued skipped retry and has not committed when its
/// lease expires. Owner B's steal must wait for A, so B never applies it again.
#[tokio::test]
async fn r10_requeued_retry_is_not_reapplied_across_a_lease_steal() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    sqlx::query("CREATE TABLE fvoci.r10_effects (event_id uuid NOT NULL)")
        .execute(&admin)
        .await
        .expect("effects table");
    sqlx::query(&format!(
        "GRANT SELECT, INSERT ON fvoci.r10_effects TO \"{}\"",
        harness.role_name.replace('"', "\"\"")
    ))
    .execute(&admin)
    .await
    .expect("grant effects");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let e = insert_test_event(&app, "E", json!({})).await.expect("E");
    ensure_consumer(&app, "r10").await.expect("ensure");
    wait_until_readable(&app, "r10", e).await;

    // Dead-letter E and skip it (as handle_failure does), then requeue.
    let a = Uuid::now_v7();
    assert!(lease_consumer(&app, "r10", a, 30).await.expect("lease a"));
    let event = read_events(&app, "r10", 10)
        .await
        .expect("read")
        .into_iter()
        .next()
        .expect("E");
    assert_eq!(
        record_failure(&app, "r10", a, e, "boom", 50, 1)
            .await
            .expect("dead"),
        1
    );
    assert!(advance_cursor(&app, "r10", a, &event.xact, event.seq)
        .await
        .expect("skip"));
    assert!(requeue(&app, "r10", e).await.expect("requeue"));
    assert!(lease_consumer(&app, "r10", a, 1)
        .await
        .expect("short lease a"));
    assert_eq!(
        claim_retries(&app, "r10", 10).await.expect("claim a").len(),
        1
    );

    let mut tx_a = app.begin().await.expect("tx a");
    sqlx::query("INSERT INTO fvoci.r10_effects (event_id) VALUES ($1)")
        .bind(e)
        .execute(&mut *tx_a)
        .await
        .expect("effect a");
    assert!(
        advance_cursor_tx(&mut tx_a, "r10", a, &event.xact, event.seq)
            .await
            .expect("advance a")
    );

    // Read-only, bounded: wait until A's lease has expired by the DB clock.
    wait_until(DISPATCHER_WAIT, || {
        let admin = admin.clone();
        Box::pin(async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT lease_until < clock_timestamp() FROM fvoci.outbox_consumers WHERE consumer = 'r10'",
            )
            .fetch_one(&admin)
            .await
            .unwrap_or(false)
        })
    })
    .await;

    let b = Uuid::now_v7();
    let app_b = app.clone();
    let (xact, seq) = (event.xact.clone(), event.seq);
    let owner_b = tokio::spawn(async move {
        if !lease_consumer(&app_b, "r10", b, 30).await.expect("lease b") {
            return (false, 0usize);
        }
        let claimed = claim_retries(&app_b, "r10", 10).await.expect("claim b");
        for retry in &claimed {
            let mut tx_b = app_b.begin().await.expect("tx b");
            sqlx::query("INSERT INTO fvoci.r10_effects (event_id) VALUES ($1)")
                .bind(retry.event_id)
                .execute(&mut *tx_b)
                .await
                .expect("effect b");
            if advance_cursor_tx(&mut tx_b, "r10", b, &xact, seq)
                .await
                .expect("advance b")
            {
                tx_b.commit().await.expect("commit b");
            } else {
                tx_b.rollback().await.expect("rollback b");
            }
        }
        (true, claimed.len())
    });

    // Read-only, bounded: B's steal is blocked behind A's lock on the lease row.
    wait_until(DISPATCHER_WAIT, || {
        let admin = admin.clone();
        Box::pin(async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM pg_locks l JOIN pg_stat_activity a ON a.pid = l.pid \
                 WHERE NOT l.granted AND a.datname = current_database())",
            )
            .fetch_one(&admin)
            .await
            .unwrap_or(false)
        })
    })
    .await;
    tx_a.commit().await.expect("commit a");
    let (b_leased, b_claimed) = owner_b.await.expect("owner b");

    let effects: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.r10_effects WHERE event_id = $1")
            .bind(e)
            .fetch_one(&admin)
            .await
            .expect("count effects");
    assert!(b_leased, "B takes over once A's expired lease is released");
    assert_eq!(b_claimed, 0, "A's resolution is visible to B");
    assert_eq!(effects, 1, "requeued PgOnly effect applied {effects} times");

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

/// R11: requeue only acts on a row the dispatcher already skipped past; a
/// requeue between the dead record and the skip would strand the event.
#[tokio::test]
async fn r11_requeue_waits_for_the_skip_and_then_redelivers() {
    let harness = TestDb::bootstrap().await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let e = insert_test_event(&app, "E", json!({})).await.expect("E");
    ensure_consumer(&app, "r11").await.expect("ensure");
    wait_until_readable(&app, "r11", e).await;
    let a = Uuid::now_v7();
    assert!(lease_consumer(&app, "r11", a, 30).await.expect("lease"));
    let event = read_events(&app, "r11", 10)
        .await
        .expect("read")
        .into_iter()
        .next()
        .expect("E");
    assert_eq!(
        record_failure(&app, "r11", a, e, "boom", 0, 1)
            .await
            .expect("dead"),
        1
    );
    assert!(!requeue(&app, "r11", e).await.expect("requeue before skip"));
    assert!(advance_cursor(&app, "r11", a, &event.xact, event.seq)
        .await
        .expect("skip advance"));
    assert!(requeue(&app, "r11", e).await.expect("requeue after skip"));
    let claimed = claim_retries(&app, "r11", 10).await.expect("claim");
    assert_eq!(claimed.len(), 1, "requeued event is delivered again");
    assert_eq!(claimed[0].event_id, e);

    app.close().await;
    harness.cleanup().await;
}

struct BatchRecordingExternal {
    name: String,
    fail_id: Option<Uuid>,
    max_attempts: i32,
    batch_sizes: Mutex<Vec<usize>>,
}

impl BatchRecordingExternal {
    fn new(name: &str, fail_id: Option<Uuid>, max_attempts: i32) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_string(),
            fail_id,
            max_attempts,
            batch_sizes: Mutex::new(Vec::new()),
        })
    }
}

impl OutboxConsumer for BatchRecordingExternal {
    fn name(&self) -> &str {
        &self.name
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::External
    }

    fn max_attempts(&self) -> i32 {
        self.max_attempts
    }

    fn deliver<'a>(
        &'a self,
        _pool: &'a PgPool,
        _lease_owner: Uuid,
        event: &'a fvoci_server::db::outbox::OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            if self.fail_id == Some(event.id) {
                Err(OutboxProcessError::Delivery(
                    "injected middle failure".into(),
                ))
            } else {
                Ok(())
            }
        })
    }

    fn deliver_batch<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        events: &'a [fvoci_server::db::outbox::OutboxEvent],
    ) -> Pin<Box<dyn Future<Output = (usize, Option<OutboxProcessError>)> + Send + 'a>> {
        self.batch_sizes
            .lock()
            .expect("batch sizes")
            .push(events.len());
        Box::pin(async move {
            let mut done = 0usize;
            for event in events {
                match self.deliver(pool, lease_owner, event).await {
                    Ok(()) => done += 1,
                    Err(err) => return (done, Some(err)),
                }
            }
            (done, None)
        })
    }
}

#[tokio::test]
async fn production_lease_delivers_more_than_one_event_per_batch() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let consumer = BatchRecordingExternal::new("batchprod", None, OUTBOX_MAX_ATTEMPTS);
    let mut ids = Vec::new();
    for n in 0..5 {
        ids.push(
            insert_test_event(&app, "test.batch", json!({ "n": n }))
                .await
                .expect("insert"),
        );
    }

    let dispatcher = spawn_outbox_dispatcher(
        OutboxDispatcherSettings {
            poll_interval: Duration::from_millis(20),
            lease_ttl: Duration::from_secs(OUTBOX_LEASE_SECS as u64),
            batch_limit: OUTBOX_DEFAULT_BATCH,
            failure_backoff: Duration::from_millis(50),
        },
        app.clone(),
        vec![consumer.clone() as Arc<dyn OutboxConsumer>],
    )
    .expect("dispatcher");

    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        let ids = ids.clone();
        Box::pin(async move {
            for id in ids {
                if !is_processed(&pool, "batchprod", id).await.unwrap_or(false) {
                    return false;
                }
            }
            true
        })
    })
    .await;

    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
    let sizes = consumer.batch_sizes.lock().expect("sizes").clone();
    assert!(
        sizes.iter().any(|&n| n > 1),
        "production 30s lease must pass more than one event to deliver_batch, got {sizes:?}"
    );

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn deliver_batch_prefix_marks_and_dead_letters_the_failed_event() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let mut ids = Vec::new();
    for n in 0..5 {
        ids.push(
            insert_test_event(&app, "test.prefix", json!({ "n": n }))
                .await
                .expect("insert"),
        );
    }
    let fail_id = ids[2];
    let consumer = BatchRecordingExternal::new("batchprefix", Some(fail_id), 3);

    let dispatcher = spawn_outbox_dispatcher(
        OutboxDispatcherSettings {
            poll_interval: Duration::from_millis(20),
            lease_ttl: Duration::from_secs(OUTBOX_LEASE_SECS as u64),
            batch_limit: OUTBOX_DEFAULT_BATCH,
            failure_backoff: Duration::from_millis(50),
        },
        app.clone(),
        vec![consumer.clone() as Arc<dyn OutboxConsumer>],
    )
    .expect("dispatcher");

    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        let ids = ids.clone();
        Box::pin(async move {
            for (i, id) in ids.iter().enumerate() {
                if i == 2 {
                    continue;
                }
                if !is_processed(&pool, "batchprefix", *id)
                    .await
                    .unwrap_or(false)
                {
                    return false;
                }
            }
            fetch_failure_state(&pool, "batchprefix", ids[2])
                .await
                .ok()
                .flatten()
                .is_some_and(|row| row.dead_at.is_some())
        })
    })
    .await;

    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");

    assert!(is_processed(&app, "batchprefix", ids[0]).await.expect("p0"));
    assert!(is_processed(&app, "batchprefix", ids[1]).await.expect("p1"));
    assert!(!is_processed(&app, "batchprefix", fail_id)
        .await
        .expect("p2"));
    assert!(is_processed(&app, "batchprefix", ids[3]).await.expect("p3"));
    assert!(is_processed(&app, "batchprefix", ids[4]).await.expect("p4"));
    let failure = fetch_failure_state(&app, "batchprefix", fail_id)
        .await
        .expect("failure")
        .expect("row");
    assert!(
        failure.dead_at.is_some(),
        "failed middle event is dead-lettered"
    );

    let last = fetch_event_by_id(&app, ids[4])
        .await
        .expect("last")
        .expect("row");
    let cursor = fetch_cursor(&admin, "batchprefix").await.expect("cursor");
    assert_eq!(cursor, Some((last.xact, last.seq)));

    let sizes = consumer.batch_sizes.lock().expect("sizes").clone();
    assert!(
        sizes.iter().any(|&n| n > 1),
        "prefix failure must be observed on a chunk larger than 1, got {sizes:?}"
    );

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

/// External consumer whose delivery of one event fails until released.
struct GatedExternal {
    name: String,
    gated: Uuid,
    open: AtomicBool,
}

impl OutboxConsumer for GatedExternal {
    fn name(&self) -> &str {
        &self.name
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::External
    }

    fn max_attempts(&self) -> i32 {
        1000
    }

    fn deliver<'a>(
        &'a self,
        _pool: &'a PgPool,
        _lease_owner: Uuid,
        event: &'a fvoci_server::db::outbox::OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            if event.id == self.gated && !self.open.load(Ordering::SeqCst) {
                Err(OutboxProcessError::Delivery("gated".into()))
            } else {
                Ok(())
            }
        })
    }
}

/// The cursor only covers a contiguous confirmed prefix: an already-processed
/// event B behind an undelivered A must not move the cursor past A. (B processed
/// while A is not happens when --recover-outbox replays a window in which B was
/// delivered; here B is marked processed directly.)
#[tokio::test]
async fn cursor_never_passes_an_undelivered_event_ahead_of_processed_ones() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let a = insert_test_event(&app, "test.gap", json!({"n": "a"}))
        .await
        .expect("a");
    let b = insert_test_event(&app, "test.gap", json!({"n": "b"}))
        .await
        .expect("b");
    let c = insert_test_event(&app, "test.gap", json!({"n": "c"}))
        .await
        .expect("c");
    ensure_consumer(&app, "gapcursor").await.expect("ensure");
    assert!(mark_processed(&app, "gapcursor", b).await.expect("mark b"));
    let consumer = Arc::new(GatedExternal {
        name: "gapcursor".into(),
        gated: a,
        open: AtomicBool::new(false),
    });
    let settings = OutboxDispatcherSettings {
        poll_interval: Duration::from_millis(20),
        lease_ttl: Duration::from_secs(OUTBOX_LEASE_SECS as u64),
        batch_limit: OUTBOX_DEFAULT_BATCH,
        failure_backoff: Duration::from_millis(50),
    };
    let dispatcher = spawn_outbox_dispatcher(
        settings.clone(),
        app.clone(),
        vec![consumer.clone() as Arc<dyn OutboxConsumer>],
    )
    .expect("dispatcher");
    // A fails at least twice (retried, not dead) while B and C sit behind it.
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        Box::pin(async move {
            fetch_failure_state(&pool, "gapcursor", a)
                .await
                .ok()
                .flatten()
                .is_some_and(|row| row.attempts >= 2 && row.dead_at.is_none())
        })
    })
    .await;
    let before_a: bool = sqlx::query_scalar(
        "SELECT (c.last_xact, c.last_seq) < (e.xact, e.seq) \
         FROM fvoci.outbox_consumers c, fvoci.events e \
         WHERE c.consumer = 'gapcursor' AND e.id = $1",
    )
    .bind(a)
    .fetch_one(&admin)
    .await
    .expect("cursor vs A");
    assert!(before_a, "cursor moved past undelivered A");
    assert!(!is_processed(&app, "gapcursor", c).await.expect("c"));

    // Restart the dispatcher (restart boundary), then let A through.
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
    consumer.open.store(true, Ordering::SeqCst);
    let dispatcher = spawn_outbox_dispatcher(
        settings,
        app.clone(),
        vec![consumer.clone() as Arc<dyn OutboxConsumer>],
    )
    .expect("dispatcher 2");
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        Box::pin(async move {
            is_processed(&pool, "gapcursor", a).await.unwrap_or(false)
                && is_processed(&pool, "gapcursor", c).await.unwrap_or(false)
        })
    })
    .await;
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join 2");
    let c_row = fetch_event_by_id(&app, c).await.expect("c row").expect("c");
    assert_eq!(
        fetch_cursor(&admin, "gapcursor").await.expect("cursor"),
        Some((c_row.xact, c_row.seq))
    );
    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

/// `fvoci-server healthcheck` with only `FVOCI_BIND` set.
fn run_healthcheck(bind: std::net::SocketAddr) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_fvoci-server"))
        .env_clear()
        .env("FVOCI_BIND", bind.to_string())
        .arg("healthcheck")
        .output()
        .expect("run healthcheck")
}

fn metric_sample<'a>(body: &'a str, name: &str) -> &'a str {
    body.lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(' '))
        .unwrap_or_else(|| panic!("{name} missing:\n{body}"))
}

/// A finite sample truncated to whole units; NaN (unknown) panics.
fn metric_value(body: &str, name: &str) -> i64 {
    let value: f64 = metric_sample(body, name).parse().expect("numeric sample");
    assert!(value.is_finite(), "{name} is {value}:\n{body}");
    value as i64
}

/// Probes on the real app role over TCP: `/ready` 200 and healthcheck exit
/// 0; `/metrics` reports the oldest undelivered event age of a registered
/// consumer; once PostgreSQL is unreachable `/ready` answers 503, the CLI
/// exits 1 and `/metrics` still answers, with the outbox gauges unknown
/// (NaN), the failure counted and the last success time kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probes_report_real_database_and_outbox_lag() {
    use fvoci_server::http::probes::{MetricsAllowList, Observability, ObservabilitySettings};

    let harness = TestDb::bootstrap().await;
    let state = project_harness::app_state(&harness.app_url).await;
    let app_pool = state.auth.db.pool.clone();
    let consumer = "probe-lag";
    ensure_consumer(&app_pool, consumer).await.expect("ensure");
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    sqlx::query(
        "INSERT INTO fvoci.events (id, verb, payload, channel, created_at) \
         VALUES ($1, 'probe', '{}', 'system', now() - interval '2 hours')",
    )
    .bind(Uuid::now_v7())
    .execute(&admin)
    .await
    .expect("old event");

    let router = fvoci_server::http::router_with_observability(
        state,
        None,
        Arc::new(fvoci_server::integrations::Integrations::disabled()),
        Arc::new(fvoci_server::identity::Identity::disabled(
            "http://localhost",
        )),
        Arc::new(Observability::new(ObservabilitySettings {
            allow: MetricsAllowList::parse(Some("127.0.0.1/32")).unwrap(),
            outbox_consumers: vec![consumer.to_string(), "probe-absent".to_string()],
            refresh_interval: Duration::ZERO,
        })),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let get = |path: &'static str| {
        let client = client.clone();
        async move {
            let response = client
                .get(format!("http://{addr}{path}"))
                .send()
                .await
                .expect("request");
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get("content-type")
                .map(|v| v.to_str().unwrap().to_string())
                .unwrap_or_default();
            (status, content_type, response.text().await.unwrap())
        }
    };

    assert_eq!(get("/ready").await.0, 200);
    assert_eq!(get("/ready").await.2, r#"{"ok":true}"#);
    let out = tokio::task::spawn_blocking(move || run_healthcheck(addr))
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");

    // The lag has no snapshot-xmin filter, so this is normally the first
    // scrape; the bounded poll only guards the refresh.
    let deadline = std::time::Instant::now() + DISPATCHER_WAIT;
    let (body, lag) = loop {
        let (status, content_type, body) = get("/metrics").await;
        assert_eq!(status, 200, "{body}");
        assert!(
            content_type.starts_with("application/openmetrics-text"),
            "{content_type}"
        );
        let lag = metric_value(&body, "fvoci_outbox_lag_seconds");
        if lag > 0 || std::time::Instant::now() > deadline {
            break (body, lag);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!((7_190..7_400).contains(&lag), "lag {lag}\n{body}");
    assert_eq!(
        metric_value(&body, "fvoci_db_metrics_refresh_failures_total"),
        0,
        "{body}"
    );
    let last_success = metric_value(&body, "fvoci_db_metrics_last_success_timestamp_seconds");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!((now - 60..=now).contains(&last_success), "{last_success}");
    // Two direct calls and the healthcheck's.
    assert!(
        body.contains(r#"fvoci_http_request_duration_seconds_count{method="GET",route="/ready",status="200"} 3"#),
        "{body}"
    );
    assert!(
        metric_value(&body, "fvoci_db_pool_max_connections") > 0,
        "{body}"
    );
    assert!(
        !body.contains(consumer),
        "consumer names stay out of labels:\n{body}"
    );

    app_pool.close().await;
    let (status, _, body) = get("/ready").await;
    assert_eq!(status, 503);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap(),
        json!({"ok": false, "checks": {"pg": false}})
    );
    let out = tokio::task::spawn_blocking(move || run_healthcheck(addr))
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let (status, _, body) = get("/metrics").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        metric_sample(&body, "fvoci_outbox_lag_seconds"),
        "NaN",
        "{body}"
    );
    assert_eq!(
        metric_sample(&body, "fvoci_outbox_xmin_stall_seconds"),
        "NaN",
        "{body}"
    );
    assert!(
        metric_value(&body, "fvoci_db_metrics_refresh_failures_total") >= 1,
        "{body}"
    );
    assert_eq!(
        metric_value(&body, "fvoci_db_metrics_last_success_timestamp_seconds"),
        last_success,
        "{body}"
    );

    server.abort();
    let _ = server.await;
    admin.close().await;
    harness.cleanup().await;
}

/// A transaction holding an xid pins the snapshot xmin, so an event committed
/// after it is not yet deliverable (`app_outbox_read` returns nothing). The
/// lag metric still counts it, as the source `lagSeconds()` did, and the
/// xmin-stall gauge reports the holder's age. Neither exposes the event's
/// workspace, actor or target, the consumer name, the database or the role.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_see_outbox_lag_and_xmin_stall_behind_an_xid_holder() {
    use fvoci_server::http::probes::{MetricsAllowList, Observability, ObservabilitySettings};

    const HOLD: Duration = Duration::from_secs(6);

    let harness = TestDb::bootstrap().await;
    let state = project_harness::app_state(&harness.app_url).await;
    let app_pool = state.auth.db.pool.clone();
    let consumer = "stall-lag";
    ensure_consumer(&app_pool, consumer).await.expect("ensure");
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    install_pin_table(&admin).await;

    let holder = begin_xmin_pin(&admin).await;
    let held_since = std::time::Instant::now();
    let (workspace_id, actor_id, target_id) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO fvoci.events \
         (id, workspace_id, actor_user_id, verb, target_type, target_id, payload, channel, created_at) \
         VALUES ($1, $2, $3, 'probe', 'document', $4, '{}', 'system', now() - interval '1 hour')",
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(actor_id)
    .bind(target_id)
    .execute(&admin)
    .await
    .expect("event behind the holder");
    let deliverable: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.app_outbox_read($1, 10)")
        .bind(consumer)
        .fetch_one(&app_pool)
        .await
        .expect("read");
    assert_eq!(
        deliverable, 0,
        "the holder must keep the event undeliverable"
    );

    let router = fvoci_server::http::router_with_observability(
        state,
        None,
        Arc::new(fvoci_server::integrations::Integrations::disabled()),
        Arc::new(fvoci_server::identity::Identity::disabled(
            "http://localhost",
        )),
        Arc::new(Observability::new(ObservabilitySettings {
            allow: MetricsAllowList::parse(Some("127.0.0.1/32")).unwrap(),
            outbox_consumers: vec![consumer.to_string()],
            refresh_interval: Duration::ZERO,
        })),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let scrape = || {
        let client = client.clone();
        async move {
            let response = client
                .get(format!("http://{addr}/metrics"))
                .send()
                .await
                .expect("scrape");
            assert_eq!(response.status().as_u16(), 200);
            response.text().await.unwrap()
        }
    };

    let body = scrape().await;
    let lag = metric_value(&body, "fvoci_outbox_lag_seconds");
    assert!((3_590..3_700).contains(&lag), "lag {lag}\n{body}");

    tokio::time::sleep(HOLD.saturating_sub(held_since.elapsed())).await;
    let body = scrape().await;
    let stall = metric_value(&body, "fvoci_outbox_xmin_stall_seconds");
    // Other tests on the shared cluster can only hold older xids, never
    // lower the maximum below this holder's age.
    assert!(stall >= 5, "stall {stall}\n{body}");
    let lag = metric_value(&body, "fvoci_outbox_lag_seconds");
    assert!((3_590..3_700).contains(&lag), "lag {lag}\n{body}");
    for secret in [
        workspace_id.to_string(),
        workspace_id.simple().to_string(),
        actor_id.to_string(),
        target_id.to_string(),
        consumer.to_string(),
        harness.db_name.clone(),
        harness.role_name.clone(),
    ] {
        assert!(!body.contains(&secret), "{secret} leaked:\n{body}");
    }

    holder.commit().await.expect("release holder");
    server.abort();
    let _ = server.await;
    app_pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

/// External consumer that takes `delay` per event, asks for `cap` events per
/// `deliver_batch` and counts each completed delivery per event.
struct SlowExternal {
    name: String,
    cap: usize,
    delay_ms: AtomicU64,
    deliveries: Mutex<HashMap<Uuid, u32>>,
}

impl SlowExternal {
    fn new(name: &str, cap: usize, delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_string(),
            cap,
            delay_ms: AtomicU64::new(delay.as_millis() as u64),
            deliveries: Mutex::new(HashMap::new()),
        })
    }

    fn count(&self, id: Uuid) -> u32 {
        self.deliveries
            .lock()
            .expect("deliveries")
            .get(&id)
            .copied()
            .unwrap_or(0)
    }

    fn total(&self) -> u32 {
        self.deliveries.lock().expect("deliveries").values().sum()
    }
}

impl OutboxConsumer for SlowExternal {
    fn name(&self) -> &str {
        &self.name
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::External
    }

    fn batch_event_cap(&self) -> usize {
        self.cap
    }

    fn deliver<'a>(
        &'a self,
        _pool: &'a PgPool,
        _lease_owner: Uuid,
        event: &'a fvoci_server::db::outbox::OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            let delay = self.delay_ms.load(Ordering::SeqCst);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            *self
                .deliveries
                .lock()
                .expect("deliveries")
                .entry(event.id)
                .or_default() += 1;
            Ok(())
        })
    }
}

async fn insert_test_events(app: &PgPool, verb: &str, n: usize) -> Vec<Uuid> {
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        ids.push(
            insert_test_event(app, verb, json!({ "n": i }))
                .await
                .expect("insert event"),
        );
    }
    ids
}

async fn all_processed(pool: &PgPool, consumer: &str, ids: &[Uuid]) -> bool {
    for id in ids {
        if !is_processed(pool, consumer, *id).await.unwrap_or(false) {
            return false;
        }
    }
    true
}

async fn processed_count(pool: &PgPool, consumer: &str, ids: &[Uuid]) -> usize {
    let mut n = 0;
    for id in ids {
        if is_processed(pool, consumer, *id)
            .await
            .expect("is_processed")
        {
            n += 1;
        }
    }
    n
}

/// Shutdown stops External delivery between events: the event in flight
/// finishes, nothing else is delivered after the cancel, the lease is
/// released, and a restarted dispatcher delivers the rest exactly once.
#[tokio::test]
async fn external_shutdown_stops_between_events_and_releases_the_lease() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let consumer = SlowExternal::new("slowshut", 1, Duration::from_secs(1));
    let ids = insert_test_events(&app, "test.slowshut", 20).await;
    ensure_consumer(&app, "slowshut").await.expect("ensure");
    wait_until_readable(&app, "slowshut", ids[19]).await;

    let dispatcher = run_dispatcher(app.clone(), consumer.clone(), 20);
    let probe = consumer.clone();
    wait_until(DISPATCHER_WAIT, || {
        let probe = probe.clone();
        Box::pin(async move { probe.total() >= 1 })
    })
    .await;
    let started = std::time::Instant::now();
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
    let elapsed = started.elapsed();
    let delivered_at_shutdown = consumer.total();
    assert!(
        elapsed < Duration::from_millis(2_500),
        "join took {elapsed:?}; {delivered_at_shutdown} events delivered by then"
    );
    assert!(
        delivered_at_shutdown <= 2,
        "only the event in flight may finish after the cancel, got {delivered_at_shutdown}"
    );
    assert!(processed_count(&app, "slowshut", &ids).await <= 2);
    let released: bool = sqlx::query_scalar(
        "SELECT lease_owner IS NULL AND lease_until IS NULL FROM fvoci.outbox_consumers WHERE consumer = 'slowshut'",
    )
    .fetch_one(&admin)
    .await
    .expect("lease row");
    assert!(released, "shutdown must release the lease");

    consumer.delay_ms.store(0, Ordering::SeqCst);
    let dispatcher = run_dispatcher(app.clone(), consumer.clone(), 20);
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        let ids = ids.clone();
        Box::pin(async move { all_processed(&pool, "slowshut", &ids).await })
    })
    .await;
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join 2");
    for id in &ids {
        assert_eq!(
            consumer.count(*id),
            1,
            "event {id} delivered more than once"
        );
    }

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

/// One dispatcher task serves every consumer in turn: a slow External
/// consumer stops starting new chunks once its lease budget is spent, so the
/// next consumer is not held for the slow consumer's whole backlog.
#[tokio::test]
async fn external_lease_budget_hands_the_dispatcher_to_the_next_consumer() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    install_delivery_table(&admin, &harness.role_name).await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let slow = SlowExternal::new("slowfair", 1, Duration::from_millis(500));
    let fast = Arc::new(PgOnlyTestConsumer::new("fastfair"));
    let ids = insert_test_events(&app, "test.fair", 20).await;
    ensure_consumer(&app, "slowfair")
        .await
        .expect("ensure slow");
    ensure_consumer(&app, "fastfair")
        .await
        .expect("ensure fast");
    wait_until_readable(&app, "slowfair", ids[19]).await;

    let dispatcher = spawn_outbox_dispatcher(
        OutboxDispatcherSettings {
            poll_interval: Duration::from_millis(20),
            lease_ttl: Duration::from_secs(2),
            batch_limit: 50,
            failure_backoff: Duration::from_millis(50),
        },
        app.clone(),
        vec![
            slow.clone() as Arc<dyn OutboxConsumer>,
            fast as Arc<dyn OutboxConsumer>,
        ],
    )
    .expect("dispatcher");
    let started = std::time::Instant::now();
    wait_until(Duration::from_secs(30), || {
        let pool = app.clone();
        Box::pin(async move { delivery_count(&pool, "fastfair").await >= 20 })
    })
    .await;
    let elapsed = started.elapsed();
    let slow_done = slow.total();
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
    assert!(
        elapsed < Duration::from_secs(5),
        "PgOnly consumer waited {elapsed:?} behind the slow External consumer ({slow_done} slow events done)"
    );
    for id in &ids {
        assert!(slow.count(*id) <= 1, "event {id} delivered more than once");
    }

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

/// Native all-or-nothing batch (like search): any chunk that contains the
/// poison event fails with `done == 0`. The failure must end up on the poison
/// event only; the innocent events before it are delivered, not dead-lettered.
struct AllOrNothingExternal {
    name: String,
    poison: Uuid,
    max_attempts: i32,
    deliveries: Mutex<HashMap<Uuid, u32>>,
    batch_sizes: Mutex<Vec<usize>>,
}

impl OutboxConsumer for AllOrNothingExternal {
    fn name(&self) -> &str {
        &self.name
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::External
    }

    fn max_attempts(&self) -> i32 {
        self.max_attempts
    }

    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        event: &'a fvoci_server::db::outbox::OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            let (done, err) = self
                .deliver_batch(pool, lease_owner, std::slice::from_ref(event))
                .await;
            match err {
                Some(err) => Err(err),
                None if done == 1 => Ok(()),
                None => Err(OutboxProcessError::Delivery("nothing delivered".into())),
            }
        })
    }

    fn deliver_batch<'a>(
        &'a self,
        _pool: &'a PgPool,
        _lease_owner: Uuid,
        events: &'a [fvoci_server::db::outbox::OutboxEvent],
    ) -> Pin<Box<dyn Future<Output = (usize, Option<OutboxProcessError>)> + Send + 'a>> {
        self.batch_sizes
            .lock()
            .expect("batch sizes")
            .push(events.len());
        Box::pin(async move {
            if events.iter().any(|event| event.id == self.poison) {
                return (
                    0,
                    Some(OutboxProcessError::Delivery(
                        "batch rejected: poison event in chunk".into(),
                    )),
                );
            }
            let mut deliveries = self.deliveries.lock().expect("deliveries");
            for event in events {
                *deliveries.entry(event.id).or_default() += 1;
            }
            (events.len(), None)
        })
    }
}

#[tokio::test]
async fn all_or_nothing_batch_failure_dead_letters_only_the_poison_event() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let ids = insert_test_events(&app, "test.poison", 6).await;
    let poison = ids[3];
    let consumer = Arc::new(AllOrNothingExternal {
        name: "aonpoison".into(),
        poison,
        max_attempts: 3,
        deliveries: Mutex::new(HashMap::new()),
        batch_sizes: Mutex::new(Vec::new()),
    });
    ensure_consumer(&app, "aonpoison").await.expect("ensure");
    wait_until_readable(&app, "aonpoison", ids[5]).await;

    let dispatcher = run_dispatcher(app.clone(), consumer.clone(), 20);
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        let tail = [ids[4], ids[5]];
        Box::pin(async move {
            all_processed(&pool, "aonpoison", &tail).await
                && fetch_failure_state(&pool, "aonpoison", poison)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|row| row.dead_at.is_some())
        })
    })
    .await;
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");

    for (i, id) in ids.iter().enumerate().take(3) {
        let failure = fetch_failure_state(&app, "aonpoison", *id)
            .await
            .expect("failure state");
        assert!(
            is_processed(&app, "aonpoison", *id).await.expect("p") && failure.is_none(),
            "innocent event {i} must be delivered with no failure row, got {failure:?}"
        );
    }
    assert!(!is_processed(&app, "aonpoison", poison)
        .await
        .expect("poison"));
    let dead = fetch_failure_state(&app, "aonpoison", poison)
        .await
        .expect("poison state")
        .expect("poison row");
    assert!(dead.dead_at.is_some());
    assert_eq!(dead.attempts, 3);
    let dead_letters: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.outbox_failures WHERE consumer = 'aonpoison' AND dead_at IS NOT NULL",
    )
    .fetch_one(&admin)
    .await
    .expect("dead letters");
    assert_eq!(dead_letters, 1, "only the poison event is dead-lettered");
    let last = fetch_event_by_id(&app, ids[5])
        .await
        .expect("last")
        .expect("row");
    assert_eq!(
        fetch_cursor(&admin, "aonpoison").await.expect("cursor"),
        Some((last.xact, last.seq))
    );
    for id in &ids {
        assert!(
            consumer
                .deliveries
                .lock()
                .expect("deliveries")
                .get(id)
                .copied()
                .unwrap_or(0)
                <= 1,
            "event {id} delivered more than once"
        );
    }
    let sizes = consumer.batch_sizes.lock().expect("sizes").clone();
    assert!(
        sizes.iter().any(|&n| n > 1),
        "the poison must first be seen inside a chunk larger than 1, got {sizes:?}"
    );

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[derive(Clone, Copy)]
enum LeaseLoss {
    /// The lease runs out while the batch is in flight (same owner).
    Expire,
    /// Another owner takes the lease while the batch is in flight.
    Steal,
}

/// Delivers every event of a batch, then loses the lease before returning.
struct LeaseLossExternal {
    name: String,
    admin: PgPool,
    loss: LeaseLoss,
    fired: AtomicBool,
    deliveries: Mutex<HashMap<Uuid, u32>>,
}

impl OutboxConsumer for LeaseLossExternal {
    fn name(&self) -> &str {
        &self.name
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::External
    }

    fn deliver<'a>(
        &'a self,
        _pool: &'a PgPool,
        _lease_owner: Uuid,
        event: &'a fvoci_server::db::outbox::OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            *self
                .deliveries
                .lock()
                .expect("deliveries")
                .entry(event.id)
                .or_default() += 1;
            Ok(())
        })
    }

    fn deliver_batch<'a>(
        &'a self,
        _pool: &'a PgPool,
        _lease_owner: Uuid,
        events: &'a [fvoci_server::db::outbox::OutboxEvent],
    ) -> Pin<Box<dyn Future<Output = (usize, Option<OutboxProcessError>)> + Send + 'a>> {
        Box::pin(async move {
            {
                let mut deliveries = self.deliveries.lock().expect("deliveries");
                for event in events {
                    *deliveries.entry(event.id).or_default() += 1;
                }
            }
            if events.len() > 1 && !self.fired.swap(true, Ordering::SeqCst) {
                let sql = match self.loss {
                    LeaseLoss::Expire => {
                        "UPDATE fvoci.outbox_consumers \
                         SET lease_until = now() - interval '1 second' \
                         WHERE consumer = $1 AND $2::uuid IS NOT NULL"
                    }
                    LeaseLoss::Steal => {
                        "UPDATE fvoci.outbox_consumers \
                         SET lease_owner = $2, lease_until = now() + interval '3 seconds' \
                         WHERE consumer = $1"
                    }
                };
                sqlx::query(sql)
                    .bind(&self.name)
                    .bind(Uuid::now_v7())
                    .execute(&self.admin)
                    .await
                    .expect("lose the lease");
            }
            (events.len(), None)
        })
    }
}

async fn assert_batch_survives_lease_loss(loss: LeaseLoss, consumer_name: &str) {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let ids = insert_test_events(&app, "test.leaseloss", 5).await;
    ensure_consumer(&app, consumer_name).await.expect("ensure");
    wait_until_readable(&app, consumer_name, ids[4]).await;
    let consumer = Arc::new(LeaseLossExternal {
        name: consumer_name.to_string(),
        admin: admin.clone(),
        loss,
        fired: AtomicBool::new(false),
        deliveries: Mutex::new(HashMap::new()),
    });

    let dispatcher = run_dispatcher(app.clone(), consumer.clone(), 20);
    let last = fetch_event_by_id(&app, ids[4])
        .await
        .expect("last")
        .expect("row");
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        let admin = admin.clone();
        let ids = ids.clone();
        let name = consumer_name.to_string();
        let last = (last.xact.clone(), last.seq);
        Box::pin(async move {
            all_processed(&pool, &name, &ids).await
                && fetch_cursor(&admin, &name).await.ok().flatten() == Some(last)
        })
    })
    .await;
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");

    assert!(
        consumer.fired.load(Ordering::SeqCst),
        "lease loss not injected"
    );
    let deliveries = consumer.deliveries.lock().expect("deliveries").clone();
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(
            deliveries.get(id).copied().unwrap_or(0),
            1,
            "event {i} delivered {:?} times after the lease was lost",
            deliveries.get(id)
        );
    }

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

/// The lease expires while a batch is in flight: every delivered event of
/// the batch is still marked and none is delivered again.
#[tokio::test]
async fn expired_lease_after_the_batch_still_marks_every_event() {
    assert_batch_survives_lease_loss(LeaseLoss::Expire, "leaseexpire").await;
}

/// Another owner takes the lease while a batch is in flight: the delivered
/// events are marked anyway, so the new owner skips them.
#[tokio::test]
async fn stolen_lease_after_the_batch_still_marks_every_event() {
    assert_batch_survives_lease_loss(LeaseLoss::Steal, "leasesteal").await;
}

/// `--recover-outbox` rewinds every cursor into the replay window. A window
/// event's mark older than the processed_events GC window (a restore from an
/// older snapshot) must survive the GC that runs at startup, or the replay
/// delivers the event again.
#[tokio::test]
async fn recover_keeps_old_window_marks_through_processed_gc() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    // `mail` is one of the consumers the processed_events GC sweeps.
    let consumer_name = "mail";
    let old = insert_test_event(&app, "test.window.old", json!({}))
        .await
        .expect("old");
    let new = insert_test_event(&app, "test.window.new", json!({}))
        .await
        .expect("new");
    ensure_consumer(&app, consumer_name).await.expect("ensure");
    wait_until_readable(&app, consumer_name, new).await;
    assert!(mark_processed(&app, consumer_name, old)
        .await
        .expect("mark old"));
    assert!(mark_processed(&app, consumer_name, new)
        .await
        .expect("mark new"));
    let new_row = fetch_event_by_id(&app, new)
        .await
        .expect("new row")
        .expect("new");
    sqlx::query(
        "UPDATE fvoci.outbox_consumers SET last_xact = $2::xid8, last_seq = $3 WHERE consumer = $1",
    )
    .bind(consumer_name)
    .bind(&new_row.xact)
    .bind(new_row.seq)
    .execute(&admin)
    .await
    .expect("cursor past both events");
    // A snapshot taken 5 days ago whose 29-day window starts 34 days ago: the
    // old event was created and delivered 33 days ago, the new one 6 days ago.
    sqlx::query(
        "UPDATE fvoci.events SET created_at = CASE WHEN id = $1 THEN now() - interval '33 days' ELSE now() - interval '6 days' END",
    )
    .bind(old)
    .execute(&admin)
    .await
    .expect("age events");
    sqlx::query(
        "UPDATE fvoci.processed_events AS p SET processed_at = e.created_at FROM fvoci.events AS e WHERE e.id = p.event_id",
    )
    .execute(&admin)
    .await
    .expect("age marks");
    let bounds: (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "SELECT now() - interval '5 days' - interval '29 days' + interval '1 hour', now() - interval '5 days'",
    )
    .fetch_one(&admin)
    .await
    .expect("recovery bounds");
    let since = bounds
        .0
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    let snapshot_at = bounds
        .1
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);

    project_harness::close_pool(app).await;
    project_harness::close_pool(admin).await;
    wait_for_client_backends_gone(&harness.admin_url, &harness.db_name).await;
    let report = recover_outbox(
        &harness.admin_url,
        RecoverOutboxOptions {
            since,
            snapshot_at,
            apply: true,
            reason: Some("test restore from an older snapshot".into()),
            acknowledge_external_replay: true,
        },
    )
    .await
    .expect("recover");
    assert!(report.applied);
    assert_eq!(report.eligible, 2, "{report:?}");

    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin after recover");
    let app = pool::connect_app(&harness.app_url)
        .await
        .expect("app after recover");
    let cancel = tokio_util::sync::CancellationToken::new();
    let deleted = fvoci_server::jobs::run_processed_gc(&app, &cancel)
        .await
        .expect("processed gc");
    assert_eq!(
        deleted, 0,
        "GC deleted a mark the recovery replay still needs"
    );
    assert!(is_processed(&app, consumer_name, old)
        .await
        .expect("old mark"));
    assert!(is_processed(&app, consumer_name, new)
        .await
        .expect("new mark"));

    // The replay then skips both events instead of delivering them again.
    wait_until_readable(&app, consumer_name, new).await;
    let replay = SlowExternal::new(consumer_name, 1, Duration::ZERO);
    let dispatcher = run_dispatcher(app.clone(), replay.clone(), 20);
    let new_row = fetch_event_by_id(&app, new)
        .await
        .expect("new row")
        .expect("new");
    wait_until(DISPATCHER_WAIT, || {
        let admin = admin.clone();
        let target = (new_row.xact.clone(), new_row.seq);
        Box::pin(async move { fetch_cursor(&admin, "mail").await.ok().flatten() == Some(target) })
    })
    .await;
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
    assert_eq!(replay.count(old), 0, "old window event delivered again");
    assert_eq!(replay.count(new), 0, "new window event delivered again");

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

/// Delivers every event of a batch, then arms a fault that fails the next
/// update of this consumer's row with a database error once: the lease
/// renewal after the batch.
struct RenewalFaultExternal {
    name: String,
    admin: PgPool,
    fired: AtomicBool,
    deliveries: Mutex<HashMap<Uuid, u32>>,
}

impl OutboxConsumer for RenewalFaultExternal {
    fn name(&self) -> &str {
        &self.name
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::External
    }

    fn deliver<'a>(
        &'a self,
        _pool: &'a PgPool,
        _lease_owner: Uuid,
        event: &'a fvoci_server::db::outbox::OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            *self
                .deliveries
                .lock()
                .expect("deliveries")
                .entry(event.id)
                .or_default() += 1;
            Ok(())
        })
    }

    fn deliver_batch<'a>(
        &'a self,
        _pool: &'a PgPool,
        _lease_owner: Uuid,
        events: &'a [fvoci_server::db::outbox::OutboxEvent],
    ) -> Pin<Box<dyn Future<Output = (usize, Option<OutboxProcessError>)> + Send + 'a>> {
        Box::pin(async move {
            {
                let mut deliveries = self.deliveries.lock().expect("deliveries");
                for event in events {
                    *deliveries.entry(event.id).or_default() += 1;
                }
            }
            if events.len() > 1 && !self.fired.swap(true, Ordering::SeqCst) {
                sqlx::query("SELECT setval('public.outbox_renewal_fault', 1, false)")
                    .execute(&self.admin)
                    .await
                    .expect("arm the renewal fault");
            }
            (events.len(), None)
        })
    }
}

/// The lease renewal after a batch fails with a database error (not a lost
/// lease): the delivered events are already marked, so the next cycle does
/// not deliver them again.
#[tokio::test]
async fn renewal_error_after_the_batch_still_marks_every_event() {
    let consumer_name = "renewfault";
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    // nextval is not rolled back with the failed statement, so an armed
    // fault (setval to 1) fires once. Unarmed, the sequence never returns 1.
    for sql in [
        "CREATE SEQUENCE public.outbox_renewal_fault START WITH 100".to_string(),
        r#"
        CREATE FUNCTION public.outbox_renewal_fault() RETURNS trigger
        LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
        BEGIN
            IF NEW.consumer = TG_ARGV[0] THEN
                IF nextval('public.outbox_renewal_fault') = 1 THEN
                    RAISE EXCEPTION 'injected lease renewal failure';
                END IF;
            END IF;
            RETURN NEW;
        END
        $$
        "#
        .to_string(),
        format!(
            "CREATE TRIGGER outbox_renewal_fault BEFORE UPDATE ON fvoci.outbox_consumers \
             FOR EACH ROW EXECUTE FUNCTION public.outbox_renewal_fault('{consumer_name}')"
        ),
    ] {
        sqlx::query(&sql)
            .execute(&admin)
            .await
            .expect("install fault");
    }
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let ids = insert_test_events(&app, "test.renewfault", 5).await;
    ensure_consumer(&app, consumer_name).await.expect("ensure");
    wait_until_readable(&app, consumer_name, ids[4]).await;
    let consumer = Arc::new(RenewalFaultExternal {
        name: consumer_name.to_string(),
        admin: admin.clone(),
        fired: AtomicBool::new(false),
        deliveries: Mutex::new(HashMap::new()),
    });

    let dispatcher = run_dispatcher(app.clone(), consumer.clone(), 20);
    let last = fetch_event_by_id(&app, ids[4])
        .await
        .expect("last")
        .expect("row");
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        let admin = admin.clone();
        let ids = ids.clone();
        let last = (last.xact.clone(), last.seq);
        Box::pin(async move {
            all_processed(&pool, consumer_name, &ids).await
                && fetch_cursor(&admin, consumer_name).await.ok().flatten() == Some(last)
        })
    })
    .await;
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");

    assert!(
        consumer.fired.load(Ordering::SeqCst),
        "renewal fault not armed"
    );
    let fault_fired: bool = sqlx::query_scalar(
        "SELECT is_called AND last_value < 100 FROM public.outbox_renewal_fault",
    )
    .fetch_one(&admin)
    .await
    .expect("fault sequence");
    assert!(fault_fired, "the armed fault must have fired");
    let deliveries = consumer.deliveries.lock().expect("deliveries").clone();
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(
            deliveries.get(id).copied().unwrap_or(0),
            1,
            "event {i} delivered {:?} times after the renewal failed",
            deliveries.get(id)
        );
    }

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

/// Delivers at most `per_call` events per `deliver_batch` call and returns
/// that count without an error; logs each call under its name.
struct PrefixExternal {
    name: String,
    per_call: usize,
    calls: Arc<Mutex<Vec<String>>>,
    deliveries: Mutex<HashMap<Uuid, u32>>,
}

impl OutboxConsumer for PrefixExternal {
    fn name(&self) -> &str {
        &self.name
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::External
    }

    fn deliver<'a>(
        &'a self,
        _pool: &'a PgPool,
        _lease_owner: Uuid,
        event: &'a fvoci_server::db::outbox::OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            *self
                .deliveries
                .lock()
                .expect("deliveries")
                .entry(event.id)
                .or_default() += 1;
            Ok(())
        })
    }

    fn deliver_batch<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        events: &'a [fvoci_server::db::outbox::OutboxEvent],
    ) -> Pin<Box<dyn Future<Output = (usize, Option<OutboxProcessError>)> + Send + 'a>> {
        self.calls.lock().expect("calls").push(self.name.clone());
        Box::pin(async move {
            let take = events.len().min(self.per_call);
            for event in &events[..take] {
                if let Err(err) = self.deliver(pool, lease_owner, event).await {
                    return (0, Some(err));
                }
            }
            (take, None)
        })
    }
}

/// A `deliver_batch` that ends early without an error (done < chunk) is not
/// a reason to end the consumer's cycle: the dispatcher passes the rest in
/// the next call right away, before it serves the next consumer.
#[tokio::test]
async fn partial_batch_without_error_continues_in_the_same_cycle() {
    let harness = TestDb::bootstrap().await;
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let partial = Arc::new(PrefixExternal {
        name: "prefixone".to_string(),
        per_call: 1,
        calls: calls.clone(),
        deliveries: Mutex::new(HashMap::new()),
    });
    let whole = Arc::new(PrefixExternal {
        name: "prefixall".to_string(),
        per_call: usize::MAX,
        calls: calls.clone(),
        deliveries: Mutex::new(HashMap::new()),
    });
    let ids = insert_test_events(&app, "test.prefix", 4).await;
    for name in ["prefixone", "prefixall"] {
        ensure_consumer(&app, name).await.expect("ensure");
        wait_until_readable(&app, name, ids[3]).await;
    }

    let dispatcher = spawn_outbox_dispatcher(
        OutboxDispatcherSettings {
            poll_interval: Duration::from_millis(20),
            lease_ttl: Duration::from_secs(2),
            batch_limit: 50,
            failure_backoff: Duration::from_millis(50),
        },
        app.clone(),
        vec![
            partial.clone() as Arc<dyn OutboxConsumer>,
            whole.clone() as Arc<dyn OutboxConsumer>,
        ],
    )
    .expect("dispatcher");
    wait_until(DISPATCHER_WAIT, || {
        let pool = app.clone();
        let ids = ids.clone();
        Box::pin(async move {
            all_processed(&pool, "prefixone", &ids).await
                && all_processed(&pool, "prefixall", &ids).await
        })
    })
    .await;
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");

    let calls = calls.lock().expect("calls").clone();
    let first_whole = calls
        .iter()
        .position(|name| name == "prefixall")
        .expect("the second consumer was served");
    assert_eq!(
        &calls[..first_whole],
        vec!["prefixone"; 4].as_slice(),
        "every partial call of one cycle comes before the next consumer: {calls:?}"
    );
    for consumer in [&partial, &whole] {
        let deliveries = consumer.deliveries.lock().expect("deliveries").clone();
        for id in &ids {
            assert_eq!(deliveries.get(id).copied(), Some(1), "{}", consumer.name);
        }
    }

    app.close().await;
    harness.cleanup().await;
}
