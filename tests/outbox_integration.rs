#![cfg(feature = "db-tests")]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use fvoci_server::db::outbox::{
    advance_cursor, advance_cursor_tx, ensure_consumer, fetch_cursor, insert_test_event,
    lease_consumer, record_failure, release_consumer, OUTBOX_MAX_ATTEMPTS,
};
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
            sqlx::query(
                r#"
                INSERT INTO fvoci.outbox_test_deliveries (consumer, event_id, verb)
                VALUES ($1, $2, $3)
                ON CONFLICT DO NOTHING
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

const DISPATCHER_WAIT: Duration = Duration::from_secs(15);

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
    tokio::time::sleep(Duration::from_millis(300)).await;
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

    tokio::time::sleep(Duration::from_millis(300)).await;
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

    tokio::time::sleep(Duration::from_millis(1200)).await;

    let owner2 = Uuid::now_v7();
    assert!(lease_consumer(&app, consumer, owner2, 30)
        .await
        .expect("lease2"));
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

    for _ in 0..OUTBOX_MAX_ATTEMPTS {
        let attempts = record_failure(&app, consumer_name, event_id, "boom", 1)
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
    let failure_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM fvoci.outbox_failures WHERE consumer = $1 AND event_id = $2",
    )
    .bind(consumer_name)
    .bind(event_id)
    .fetch_one(&admin)
    .await
    .expect("failures");
    assert_eq!(failure_count, 1);

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

    let failing = Arc::new(FailingConsumer::new(consumer_name, 0));
    let dispatcher = run_dispatcher(app.clone(), failing, 20);
    wait_until(DISPATCHER_WAIT, || {
        let pool = admin.clone();
        Box::pin(async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM fvoci.outbox_failures WHERE consumer = $1 AND event_id = $2",
            )
            .bind(consumer_name)
            .bind(event_id)
            .fetch_optional(&pool)
            .await
            .expect("count")
            .unwrap_or(1)
                == 0
        })
    })
    .await;

    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");
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
