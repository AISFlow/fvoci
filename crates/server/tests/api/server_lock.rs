//! One process is the server of a database: a second waits for the lock of the server
//! and does not start while the first holds it, a restarted one starts once the first
//! let it go, and one that loses it stops its housekeeping and goes down.

use std::time::{Duration, Instant};

use fvoci_server::Server;
use fvoci_server::app::ReadinessCheck;
use fvoci_server::auth::now_millis;
use fvoci_store::DbType;

use crate::common::Setup;

/// A second start waits as long as it is told to, and is refused with who holds the
/// lock; once the first server let it go, a start that does not wait takes it.
#[tokio::test]
async fn a_second_server_waits_for_the_lock_and_is_refused() {
    let ts = Setup::native().start().await;
    assert!(ts.app.holds_server_lock());
    let mut config = ts.app.config().clone();
    config.server_lock_wait_seconds = 1;
    let started = Instant::now();
    let refusal = Server::new(config.clone(), "")
        .await
        .expect_err("the first server holds the lock");
    assert!(started.elapsed() >= Duration::from_secs(1), "it waited");
    let refusal = format!("{refusal:#}");
    assert!(refusal.contains("fvoci-server process"), "{refusal}");
    assert!(refusal.contains("server_lock_wait_seconds"), "{refusal}");

    // Without any wait: a lock still held would refuse it at once.
    let kept = ts.stop_keeping_data().await;
    config.server_lock_wait_seconds = 0;
    let restarted = Server::new(config, "").await.expect("the lock was let go");
    assert!(restarted.app().holds_server_lock());
    restarted.shutdown().await;
    kept.remove().await;
}

/// PostgreSQL: a server whose lock's connection is cut is not ready, refuses its
/// housekeeping, and stops serving.
#[tokio::test]
async fn a_server_that_loses_its_lock_stops() {
    let ts = Setup::native().start().await;
    if ts.app.store().db_type() != DbType::Postgres {
        ts.teardown().await;
        return;
    }
    let cut = ts
        .app
        .store()
        .execute_unchecked(
            "SELECT pg_terminate_backend(pid) FROM pg_locks WHERE locktype = 'advisory' \
             AND classid = 1719037795::oid AND objid = hashtext(current_schema())::oid",
        )
        .await
        .expect("the lock's connection is cut");
    assert_eq!(cut, 1);
    tokio::time::timeout(Duration::from_secs(30), async {
        while ts.app.holds_server_lock() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the loss is seen");
    let refused = ts
        .app
        .housekeeping()
        .remove_doomed_objects(now_millis())
        .await
        .expect_err("no housekeeping without the lock");
    assert_eq!(refused.status(), 503);
    assert_eq!(ts.app.readiness().await.database, ReadinessCheck::Failing);
    let served = ts.stop().await;
    assert!(
        format!("{:#}", served.expect_err("the server went down")).contains("lock"),
        "the server says why it went down"
    );
}
