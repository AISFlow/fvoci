#![cfg(feature = "db-tests")]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fvoci_server::db::outbox::{
    advance_cursor, advance_cursor_tx, claim_retries, ensure_consumer, fetch_cursor,
    fetch_failure_state, insert_test_event, is_outbox_xid_epoch_mismatch, lease_consumer,
    read_events, record_failure, release_consumer, requeue, OUTBOX_MAX_ATTEMPTS,
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

    let leftover: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM fvoci.outbox_failures WHERE consumer = 'exactly-once' AND event_id = $1",
    )
    .bind(event_id)
    .fetch_one(&admin)
    .await
    .expect("cleared failure");
    assert_eq!(leftover, 0);

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

    app.close().await;
    admin.close().await;
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
