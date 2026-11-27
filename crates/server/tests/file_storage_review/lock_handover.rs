//! The lock of the server, handed over: one SQLite database under another name, a
//! removal that learns the lock was lost, and a shutdown with a write still under way.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use fvoci_model::{Card, ContainerVisibility, FileRecord, new_id};
use fvoci_server::Server;
use fvoci_server::app::{App, ChangeNotification};
use fvoci_server::auth::now_millis;
use fvoci_server::storage::WRITING_DIR;
use fvoci_store::{Access, Call, DbType, Observer, Store};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::oneshot;

use crate::common::{Setup, TEST_WORKSPACE_ID};
use crate::recorded;

/// Returns once a file is being written under `directory`.
async fn wait_for_partial_file(directory: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(mut entries) = tokio::fs::read_dir(directory).await
                && entries
                    .next_entry()
                    .await
                    .expect("directory read")
                    .is_some()
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the active write has a partial file");
}

/// SQLite follows a symbolic link to the same database: a start under the link's name
/// is refused the lock, and does not sweep away the first server's file being written.
#[cfg(unix)]
#[tokio::test]
async fn a_sqlite_database_symlink_cannot_bypass_the_server_lock() {
    if fvoci_store::testing::db_type_of_the_tests() != DbType::Sqlite {
        return;
    }
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let config = ts.app.config();
    let database = config
        .database_connection
        .split('?')
        .next()
        .expect("a filename");
    let alias = PathBuf::from(database).with_extension("alias.db");
    std::os::unix::fs::symlink(database, &alias).expect("an alias of the same database");
    let mut aliased = config.clone();
    aliased.database_connection = format!("{}?busy_timeout_ms=5000", alias.display());
    aliased.server_lock_wait_seconds = 0;
    let app = ts.app.clone();
    let (mut writer, mut reader) = tokio::io::duplex(64);
    let saving =
        tokio::spawn(async move { app.save_file(&mut reader, &board.id, "in-flight.txt").await });
    writer.write_all(b"in-flight bytes").await.expect("sent");
    wait_for_partial_file(&Path::new(&config.file_storage_path).join(WRITING_DIR)).await;

    let second = Server::new(aliased, "").await;
    let refused = second
        .as_ref()
        .is_err_and(|refusal| format!("{refusal:#}").contains("fvoci-server process"));
    writer.shutdown().await.expect("the first write can finish");
    let saved = saving.await.expect("the first write returns");
    if let Ok(second) = second {
        second.shutdown().await;
    }
    ts.teardown().await;
    assert!(
        refused && saved.is_ok(),
        "the same SQLite database must exclude an aliased startup and preserve the active write: \
         refused={refused}, saved={saved:?}"
    );
}

/// Pauses a removal run after its second read: the list of due objects is the first,
/// `held_objects` the second. Called on a blocking task of its own, so that the server
/// and the watcher of its lock go on meanwhile.
#[derive(Debug)]
struct AfterHoldQuery {
    reads: AtomicUsize,
    reached: Mutex<Option<oneshot::Sender<()>>>,
    resume: Mutex<mpsc::Receiver<()>>,
}

impl Observer for AfterHoldQuery {
    fn called(&self, call: Call) {
        if !matches!(call.access, Access::Read) || self.reads.fetch_add(1, Ordering::SeqCst) != 1 {
            return;
        }
        if let Some(reached) = self.reached.lock().expect("gate lock").take() {
            let _ = reached.send(());
        }
        self.resume
            .lock()
            .expect("gate lock")
            .recv_timeout(Duration::from_secs(40))
            .expect("the review releases the old worker");
    }

    fn waited_for_connection(&self, _: DbType, _: Duration) {}
}

/// The next server removes the previous holder's job and records a replacement at its place.
async fn replace_as_the_next_server(next: &App, file: FileRecord) -> FileRecord {
    next.housekeeping()
        .remove_doomed_objects(now_millis())
        .await
        .expect("the next holder removes the original job");
    next.store()
        .mark_writing(&file.path, i64::MAX)
        .await
        .expect("marked");
    let size = next
        .files()
        .write(&file.path, &mut &b"replacement bytes"[..])
        .await
        .expect("replacement written");
    let replacement = FileRecord {
        id: new_id(),
        size: i64::try_from(size).expect("a small file"),
        created_at: now_millis(),
        ..file
    };
    assert!(
        next.store()
            .record_written_file(&replacement)
            .await
            .expect("recorded"),
        "the new holder records its replacement"
    );
    replacement
}

/// A removal paused past `held_objects` while its server learns that the lock is
/// lost, and the next server records a replacement at the place: resumed, the old
/// worker removes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_worker_that_observed_lock_loss_cannot_delete_the_new_holders_file() {
    if fvoci_store::testing::db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::native().start().await.init_basic().await;
    let file = recorded(&ts).await;
    ts.app
        .housekeeping()
        .store()
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("the original is doomed");
    // Asked before the gate goes in, so that it is not one of the reads it counts.
    assert!(
        !ts.app
            .store()
            .other_spellings_stored()
            .await
            .expect("paths checked"),
        "all paths are canonical"
    );
    let independent = Store::connect(
        ts.app.store().db_type(),
        &ts.app.config().database_connection,
    )
    .await
    .expect("a separate pool controls the handover");
    let (reached, paused) = oneshot::channel();
    let (resume, wait) = mpsc::channel();
    assert!(
        ts.app.store().observe(Box::new(AfterHoldQuery {
            reads: AtomicUsize::new(0),
            reached: Mutex::new(Some(reached)),
            resume: Mutex::new(wait),
        })),
        "the gate is installed"
    );
    let old = ts.app.housekeeping();
    let runtime = tokio::runtime::Handle::current();
    let removing = tokio::task::spawn_blocking(move || {
        runtime.block_on(old.remove_doomed_objects(now_millis()))
    });
    tokio::time::timeout(Duration::from_secs(10), paused)
        .await
        .expect("the old worker reaches its hold query")
        .expect("the gate is reached");
    independent
        .execute_unchecked(
            "SELECT pg_terminate_backend(pid) FROM pg_locks WHERE locktype = 'advisory' \
             AND classid = 1719037795::oid AND objid = hashtext(current_schema())::oid",
        )
        .await
        .expect("only this test's lock connection is cut");
    tokio::time::timeout(Duration::from_secs(20), async {
        while ts.app.holds_server_lock() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the old server has observed its loss");
    let mut config = ts.app.config().clone();
    config.server_lock_wait_seconds = 0;
    let next = Server::new(config, "")
        .await
        .expect("the next server takes the lock");
    let next_app = next.app().scoped(ts.app.store().tenant());
    let replacement = replace_as_the_next_server(&next_app, file).await;
    resume
        .send(())
        .expect("the old worker resumes after observed loss");
    let removed = removing.await.expect("the old worker returns");
    let still_recorded = next_app
        .store()
        .get_file_record(&replacement.container_id, &replacement.stored_name)
        .await
        .is_ok();
    let still_present = next
        .app()
        .files()
        .exists(&replacement.path)
        .await
        .expect("read");
    next.shutdown().await;
    independent.close().await;
    let stopped = ts.stop().await;
    assert!(stopped.is_err(), "the old server stops for lock loss");
    assert!(
        removed.is_err(),
        "the old worker's run ends refused: {removed:?}"
    );
    assert!(still_recorded, "the replacement's live record remains");
    assert!(
        still_present,
        "a worker that knows its lock is lost must preserve the new holder's bytes"
    );
}

/// A shutdown whose pool does not close in time, because a transaction of a write
/// still has its connection, keeps the lock of the server until that write ends: a
/// restart meanwhile is refused, and one after it starts. The write is held up by a
/// lock of the `blocks` table taken from another pool for 30 seconds, longer than the
/// 15 of shutdown's waits, and is tracked as the work of the handlers is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_cannot_release_the_server_lock_before_an_old_write_commits() {
    if fvoci_store::testing::db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let user = ts.client.me().await.id;
    let mut config = ts.app.config().clone();
    let tenant = ts.app.store().tenant();
    let kept = ts.stop_keeping_data().await;
    let server = Server::new(config.clone(), "")
        .await
        .expect("the server restarts");
    let independent = Store::connect(DbType::Postgres, &config.database_connection)
        .await
        .expect("a separate pool controls the table lock");
    let locking = independent.clone();
    let blocker = tokio::spawn(async move {
        locking
            .execute_unchecked(
                "DO $$ BEGIN LOCK TABLE blocks IN ACCESS EXCLUSIVE MODE; \
                 PERFORM pg_sleep(30); END $$",
            )
            .await
    });
    wait_for_table_lock(&independent, TableLock::Held).await;
    let app = server.app().scoped(tenant);
    let board_id = board.id.clone();
    let (completed, mut writing) = oneshot::channel();
    server.app().spawn_background(async move {
        let written = app
            .create_card(
                Card {
                    title: "write surviving shutdown".to_owned(),
                    ..Card::default()
                },
                &board_id,
                &user,
                ChangeNotification::default(),
            )
            .await;
        let _ = completed.send(written);
    });
    wait_for_table_lock(&independent, TableLock::Awaited).await;
    server.shutdown().await;
    assert!(writing.try_recv().is_err(), "the write is still under way");

    config.server_lock_wait_seconds = 0;
    let refusal = Server::new(config.clone(), "")
        .await
        .expect_err("the lock is kept while the old write is under way");
    assert!(
        format!("{refusal:#}").contains("fvoci-server process"),
        "{refusal:#}"
    );
    config.server_lock_wait_seconds = 60;
    let next = Server::new(config, "")
        .await
        .expect("the next holder starts once the write ended");
    let written = writing
        .try_recv()
        .expect("the old write ended before the lock went");
    assert!(next.app().holds_server_lock());
    blocker
        .await
        .expect("the blocker returns")
        .expect("the table lock is released");
    let card = written.expect("the old write commits under the old server's lock");
    assert!(
        next.app()
            .scoped(tenant)
            .store()
            .get_block(&card.id)
            .await
            .is_ok(),
        "what the old server committed under its lock is there"
    );
    next.shutdown().await;
    independent.close().await;
    kept.remove().await;
}

/// What is waited for of the lock of the `blocks` table.
#[derive(Debug, Clone, Copy)]
enum TableLock {
    /// The blocker holds it.
    Held,
    /// A write waits for it.
    Awaited,
}

async fn wait_for_table_lock(store: &Store, lock: TableLock) {
    let condition = match lock {
        TableLock::Held => "mode = 'AccessExclusiveLock' AND granted",
        TableLock::Awaited => "NOT granted",
    };
    let query = format!(
        "SELECT 1 WHERE EXISTS (SELECT 1 FROM pg_locks \
         WHERE relation = 'blocks'::regclass AND {condition})"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if store
                .execute_unchecked(&query)
                .await
                .expect("this schema's locks are read")
                > 0
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the transaction reaches its table lock");
}
