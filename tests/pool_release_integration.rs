#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! The app pool (`db::pool::connect_app*`) never hands out a connection that
//! is still inside a server-side transaction. sqlx 0.8's `Transaction::begin`
//! counts the transaction only after `BEGIN` completes, so a begin future
//! dropped in between (a client abort) returns its connection with the
//! transaction open and no `ROLLBACK` queued. In CI run 36472820268 an aborted
//! `GET /api/v1/me/workspaces` leaked `pool.begin()`'s `BEGIN` that way; a later
//! `begin_read` on that connection failed and aborted it, and every later user
//! of the connection got `current transaction is aborted`. Run as the
//! NOSUPERUSER app role against real PostgreSQL.

#[path = "support/project_harness.rs"]
mod project_harness;

use std::io::Write;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fvoci_server::db::context::begin_read;
use fvoci_server::db::pool;
use project_harness::{admin_pool, close_pool, TestDb};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

const RELEASE_EVENT: &str = "db.pool.release_in_transaction";
const WAIT: Duration = Duration::from_secs(10);

/// Warn-level log lines emitted on this test's thread. `#[tokio::test]` runs a
/// current-thread runtime, so the pool's spawned release task logs here too.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Captured {
    fn lines(&self) -> Vec<String> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn capture_warnings() -> (Captured, tracing::subscriber::DefaultGuard) {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    (captured, guard)
}

/// The release check logged once per closed connection, with its fixed event
/// name and no SQL, and sqlx logged nothing of its own (its `after_release`
/// error path).
fn assert_release_warnings(logs: &Captured, states: &[&str]) {
    let lines = logs.lines();
    assert_eq!(lines.len(), states.len(), "warn lines: {lines:#?}");
    for (line, state) in lines.iter().zip(states) {
        assert!(line.contains(RELEASE_EVENT), "{line}");
        assert!(line.contains(&format!("state=\"{state}\"")), "{line}");
        for sql in ["BEGIN", "SELECT", "statement_timestamp"] {
            assert!(!line.contains(sql), "{line}");
        }
    }
}

async fn backend_pid<'c, E>(executor: E) -> i32
where
    E: sqlx::Executor<'c, Database = sqlx::Postgres>,
{
    sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(executor)
        .await
        .expect("backend pid")
}

async fn wait_backend_gone(admin: &PgPool, pid: i32) {
    let deadline = Instant::now() + WAIT;
    loop {
        let present: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE pid = $1)")
                .bind(pid)
                .fetch_one(admin)
                .await
                .unwrap();
        if !present {
            return;
        }
        assert!(Instant::now() < deadline, "backend {pid} still running");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn wait_backend_state(admin: &PgPool, pid: i32, state: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        let current: Option<String> =
            sqlx::query_scalar("SELECT state FROM pg_stat_activity WHERE pid = $1")
                .bind(pid)
                .fetch_optional(admin)
                .await
                .unwrap()
                .flatten();
        if current.as_deref() == Some(state) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "backend {pid} state {current:?}, want {state}"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

async fn open_transactions(admin: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity
         WHERE datname = current_database() AND state LIKE 'idle in transaction%'",
    )
    .fetch_one(admin)
    .await
    .unwrap()
}

/// The pool's next users get a fresh backend outside any transaction: a plain
/// query (inside a leaked transaction it takes the snapshot), then a read
/// transaction (inside a leaked READ COMMITTED transaction with a snapshot its
/// isolation level fails, as in CI, and aborts it), then a plain query.
async fn assert_next_users_clean(pool: &PgPool, leaked_pid: i32) {
    let pid = backend_pid(pool).await;
    let mut tx = begin_read(pool)
        .await
        .expect("begin_read after a leaked transaction");
    let one: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(one, 1);
    tx.commit().await.unwrap();
    let one: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(pool)
        .await
        .expect("query after a leaked transaction");
    assert_eq!(one, 1);
    assert_ne!(pid, leaked_pid, "the leaked backend was handed out again");
}

/// `BEGIN` statements of `pool.begin()` (CI's leak) and of `begin_read`.
const LEAKED_BEGINS: [&str; 2] = ["BEGIN", "BEGIN ISOLATION LEVEL REPEATABLE READ, READ ONLY"];

#[tokio::test]
async fn released_open_transaction_is_closed_not_reused() {
    let harness = TestDb::bootstrap().await;
    let admin = admin_pool(&harness).await;
    // One slot: every acquire below waits for the previous release to finish.
    let pool = pool::connect_app_with_max(&harness.app_url, 1)
        .await
        .expect("app pool");
    let (logs, _guard) = capture_warnings();

    for begin in LEAKED_BEGINS {
        let mut conn = pool.acquire().await.unwrap();
        let leaked_pid = backend_pid(&mut *conn).await;
        // Outside sqlx's transaction API its depth stays 0, so the drop below
        // queues no ROLLBACK: the state a cancelled begin leaves.
        sqlx::raw_sql(begin).execute(&mut *conn).await.unwrap();
        sqlx::query("SELECT 1").execute(&mut *conn).await.unwrap();
        drop(conn);

        assert_next_users_clean(&pool, leaked_pid).await;
        wait_backend_gone(&admin, leaked_pid).await;
        assert_eq!(open_transactions(&admin).await, 0);
    }
    assert_release_warnings(&logs, &["open", "open"]);

    close_pool(pool).await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn released_aborted_transaction_is_closed_not_reused() {
    let harness = TestDb::bootstrap().await;
    let admin = admin_pool(&harness).await;
    let pool = pool::connect_app_with_max(&harness.app_url, 1)
        .await
        .expect("app pool");
    let (logs, _guard) = capture_warnings();

    let mut conn = pool.acquire().await.unwrap();
    let leaked_pid = backend_pid(&mut *conn).await;
    sqlx::raw_sql("BEGIN").execute(&mut *conn).await.unwrap();
    let error = sqlx::raw_sql("SELECT 1/0")
        .execute(&mut *conn)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("division by zero"), "{error}");
    drop(conn);

    assert_next_users_clean(&pool, leaked_pid).await;
    wait_backend_gone(&admin, leaked_pid).await;
    assert_eq!(open_transactions(&admin).await, 0);
    assert_release_warnings(&logs, &["aborted"]);

    close_pool(pool).await;
    admin.close().await;
    harness.cleanup().await;
}

/// TCP proxy in front of PostgreSQL. Once armed with a trigger, the first
/// client chunk that carries it turns `hold` on before it is forwarded, so the
/// server runs that statement while its reply waits here until `hold` is off.
struct ReplyHoldingProxy {
    addr: SocketAddr,
    trigger: Arc<Mutex<Option<&'static [u8]>>>,
    hold: Arc<watch::Sender<bool>>,
}

impl ReplyHoldingProxy {
    async fn spawn(upstream: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let trigger: Arc<Mutex<Option<&'static [u8]>>> = Arc::default();
        let hold = Arc::new(watch::Sender::new(false));
        let (proxy_trigger, proxy_hold) = (trigger.clone(), hold.clone());
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let Ok(server) = TcpStream::connect(upstream.as_str()).await else {
                    return;
                };
                let (mut client_read, mut client_write) = client.into_split();
                let (mut server_read, mut server_write) = server.into_split();
                let (trigger, hold) = (proxy_trigger.clone(), proxy_hold.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 16 * 1024];
                    loop {
                        let n = match client_read.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        let fired = {
                            let mut armed = trigger.lock().unwrap();
                            let hit =
                                armed.is_some_and(|t| buf[..n].windows(t.len()).any(|w| w == t));
                            if hit {
                                *armed = None;
                            }
                            hit
                        };
                        if fired {
                            hold.send_replace(true);
                        }
                        if server_write.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                    let _ = server_write.shutdown().await;
                });
                let mut held = proxy_hold.subscribe();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 16 * 1024];
                    loop {
                        let n = match server_read.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        if held.wait_for(|on| !*on).await.is_err() {
                            break;
                        }
                        if client_write.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                    let _ = client_write.shutdown().await;
                });
            }
        });
        Self {
            addr,
            trigger,
            hold,
        }
    }

    fn arm(&self, trigger: &'static [u8]) {
        *self.trigger.lock().unwrap() = Some(trigger);
    }

    fn release(&self) {
        self.hold.send_replace(false);
    }
}

/// Drops `begin` once PostgreSQL has run its `BEGIN` on `pid` (the reply held
/// by the proxy): a client abort while the handler is inside
/// `Transaction::begin`.
async fn cancel_after_begin_ran<F, T>(admin: &PgPool, pid: i32, begin: F)
where
    F: std::future::Future<Output = Result<T, sqlx::Error>>,
{
    tokio::time::timeout(WAIT, async {
        tokio::select! {
            begun = begin => {
                panic!("begin finished while its reply was held: {:?}", begun.map(|_| ()))
            }
            () = wait_backend_state(admin, pid, "idle in transaction") => {}
        }
    })
    .await
    .expect("BEGIN reached the server");
}

/// A real client abort, as in CI: the begin future is dropped after
/// PostgreSQL ran its `BEGIN` but before the reply arrived.
#[tokio::test]
async fn cancelled_begin_does_not_leak_its_transaction() {
    let harness = TestDb::bootstrap().await;
    let admin = admin_pool(&harness).await;
    let mut url = url::Url::parse(&harness.app_url).unwrap();
    let upstream = format!("{}:{}", url.host_str().unwrap(), url.port().unwrap());
    let proxy = ReplyHoldingProxy::spawn(upstream).await;
    url.set_port(Some(proxy.addr.port())).unwrap();
    // The proxy reads the protocol, so keep it in clear text.
    url.query_pairs_mut().append_pair("sslmode", "disable");
    let pool = pool::connect_app_with_max(url.as_str(), 1)
        .await
        .expect("app pool through proxy");
    let (logs, _guard) = capture_warnings();

    // `pool.begin()` (CI's `list_workspaces_for_user`): simple-query `BEGIN`.
    let leaked_pid = backend_pid(&pool).await;
    proxy.arm(b"BEGIN\0");
    cancel_after_begin_ran(&admin, leaked_pid, pool.begin()).await;
    proxy.release();
    assert_next_users_clean(&pool, leaked_pid).await;
    wait_backend_gone(&admin, leaked_pid).await;

    let leaked_pid = backend_pid(&pool).await;
    proxy.arm(b"BEGIN ISOLATION LEVEL REPEATABLE READ, READ ONLY\0");
    cancel_after_begin_ran(&admin, leaked_pid, begin_read(&pool)).await;
    proxy.release();
    assert_next_users_clean(&pool, leaked_pid).await;
    wait_backend_gone(&admin, leaked_pid).await;

    assert_eq!(open_transactions(&admin).await, 0);
    assert_release_warnings(&logs, &["open", "open"]);

    close_pool(pool).await;
    admin.close().await;
    harness.cleanup().await;
}

/// No false positives: transactions sqlx ended (committed, rolled back, or
/// dropped with a queued ROLLBACK, also after a failed statement), failed
/// statements outside a transaction and a cancelled query all hand the same
/// backend back, with nothing logged.
#[tokio::test]
async fn ended_transactions_keep_their_connection() {
    let harness = TestDb::bootstrap().await;
    let pool = pool::connect_app_with_max(&harness.app_url, 1)
        .await
        .expect("app pool");
    let (logs, _guard) = capture_warnings();
    let pid = backend_pid(&pool).await;

    for _ in 0..25 {
        let mut tx = begin_read(&pool).await.unwrap();
        sqlx::query("SELECT 1").execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();

        // Dropped: sqlx queues a ROLLBACK that the release check flushes first.
        let mut tx = begin_read(&pool).await.unwrap();
        sqlx::query("SELECT 1").execute(&mut *tx).await.unwrap();
        drop(tx);

        let tx = pool.begin().await.unwrap();
        tx.rollback().await.unwrap();

        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT 1/0")
            .execute(&mut *tx)
            .await
            .unwrap_err();
        drop(tx);

        sqlx::query("SELECT 1/0").execute(&pool).await.unwrap_err();
        sqlx::raw_sql("SELECT 1").execute(&pool).await.unwrap();
    }

    let mut conn = pool.acquire().await.unwrap();
    tokio::select! {
        done = sqlx::query("SELECT pg_sleep(0.3)").execute(&mut *conn) => {
            panic!("pg_sleep finished early: {done:?}")
        }
        () = tokio::time::sleep(Duration::from_millis(30)) => {}
    }
    drop(conn);

    assert_eq!(backend_pid(&pool).await, pid);
    let lines = logs.lines();
    assert!(lines.is_empty(), "warn lines: {lines:#?}");

    close_pool(pool).await;
    harness.cleanup().await;
}
