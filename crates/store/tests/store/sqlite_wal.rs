//! An SQLite file is kept with a write-ahead log: who reads does not wait for who
//! writes, nor the other way round, what was committed is there when the file is
//! opened again (after a server that was not closed, too), and a copy taken through
//! SQLite while the store works has the last commit.
//!
//! With the rollback journal a commit held the whole file: a read begun meanwhile
//! waited, and failed with "database is locked" once the busy timeout was over. A
//! commit waited likewise for every read under way. On a busy machine that was seconds
//! of nothing, in every request at once.

use std::path::{Path, PathBuf};

use fvoci_model::{Workspace, new_id};
use fvoci_store::testing::TestDatabase;
use fvoci_store::{DbType, Store};
use sqlx::Connection as _;
use sqlx::sqlite::SqliteConnection;

use crate::common::{empty_database, organization};

/// How long a statement of these stores waits for the file, in milliseconds: short, so
/// that a wait which should not be is a failure soon.
const BUSY_TIMEOUT_MS: u32 = 300;

/// A store on an SQLite file of the test's own, with the organization and the
/// workspace made in it. `None` where the tests run against PostgreSQL.
async fn on_a_file() -> Option<Scene> {
    let database = empty_database().await;
    let path = database.sqlite_path()?.to_path_buf();
    let unset = connect(&path).await;
    let (org_id, workspace_id) = organization(&unset, "").await;
    let store = unset.for_org(&org_id).expect("an identifier");
    Some(Scene {
        database,
        path,
        store,
        org_id,
        workspace_id,
    })
}

/// A store on a file of the test's own, working in the file's one organization.
struct Scene {
    database: TestDatabase,
    path: PathBuf,
    store: Store,
    org_id: String,
    /// The organization's first workspace.
    workspace_id: String,
}

impl Scene {
    /// A store on `path`, a file with this one's organization in it.
    async fn open(&self, path: &Path) -> Store {
        connect(path)
            .await
            .for_org(&self.org_id)
            .expect("an identifier")
    }
}

async fn connect(path: &Path) -> Store {
    let connection = format!("{}?busy_timeout_ms={BUSY_TIMEOUT_MS}", path.display());
    Store::connect(DbType::Sqlite, &connection)
        .await
        .expect("a store opens the file")
}

/// A connection to the file past the store, as another process would have one.
async fn outside(path: &Path) -> SqliteConnection {
    SqliteConnection::connect(&path.display().to_string())
        .await
        .expect("the database file opens")
}

/// How the file keeps what is being written, as whoever opens it finds it.
async fn journal_of(path: &Path) -> String {
    let mut looking = outside(path).await;
    let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut looking)
        .await
        .expect("the mode");
    looking.close().await.expect("the connection closes");
    journal
}

async fn run(connection: &mut SqliteConnection, statement: &'static str) {
    sqlx::raw_sql(statement)
        .execute(connection)
        .await
        .unwrap_or_else(|error| panic!("{statement}: {error}"));
}

/// How many workspaces of that name the connection reads, or why it reads none.
async fn workspaces_named(connection: &mut SqliteConnection, name: &str) -> Result<i64, String> {
    sqlx::query_scalar("SELECT COUNT(*) FROM workspaces WHERE name = ?")
        .bind(name)
        .fetch_one(connection)
        .await
        .map_err(|error| error.to_string())
}

/// A workspace of the store's organization, stored: its identifier.
async fn another_workspace(store: &Store, title: &str) -> String {
    let workspace = Workspace {
        id: new_id(),
        title: title.to_owned(),
        ..Workspace::default()
    };
    store
        .create_workspace(&workspace)
        .await
        .expect("the workspace is stored");
    workspace.id
}

fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// A writer that holds the file as a commit does (`BEGIN EXCLUSIVE` takes at once what
/// a commit of the rollback journal takes) keeps no read waiting, and what it has not
/// committed is not read.
#[tokio::test]
async fn a_read_does_not_wait_for_a_writer_that_holds_the_file() {
    let Some(scene) = on_a_file().await else {
        return;
    };
    let (store, path, workspace_id) = (&scene.store, &scene.path, &scene.workspace_id);
    let mut writer = outside(path).await;
    run(&mut writer, "BEGIN EXCLUSIVE").await;
    run(&mut writer, "UPDATE workspaces SET name = 'held'").await;

    let read = store.get_workspace(workspace_id).await;
    let title = read.expect("read while the writer holds the file").title;
    assert_ne!(title, "held", "what is not committed is not read");

    run(&mut writer, "COMMIT").await;
    let read = store.get_workspace(workspace_id).await.expect("read");
    assert_eq!(read.title, "held");
    writer.close().await.expect("the connection closes");
    store.close().await;
    scene.database.remove().await;
}

/// Writers take turns with the log too: one that waits its whole time is told so by
/// an error that says the database was busy, which nothing else is.
#[tokio::test]
async fn a_write_that_waits_its_whole_time_fails_as_busy_and_nothing_else_does() {
    let Some(scene) = on_a_file().await else {
        return;
    };
    let (store, path) = (&scene.store, &scene.path);
    let mut writer = outside(path).await;
    run(&mut writer, "BEGIN IMMEDIATE").await;
    run(&mut writer, "UPDATE workspaces SET name = name").await;

    let waited = store
        .create_workspace(&Workspace {
            id: new_id(),
            ..Workspace::default()
        })
        .await
        .expect_err("the file is held");
    assert!(waited.is_busy(), "{waited:?}");

    run(&mut writer, "COMMIT").await;
    writer.close().await.expect("the connection closes");
    let missing = store.get_workspace(&new_id()).await.expect_err("none");
    assert!(!missing.is_busy(), "{missing:?}");
    store.close().await;
    scene.database.remove().await;
}

/// A read under way (a transaction that has read, and is not over) keeps no commit
/// waiting, and goes on reading what was there when it began.
#[tokio::test]
async fn a_commit_does_not_wait_for_a_read_under_way() {
    let Some(scene) = on_a_file().await else {
        return;
    };
    let (store, path) = (&scene.store, &scene.path);
    let mut reader = outside(path).await;
    run(&mut reader, "BEGIN").await;
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspaces")
        .fetch_one(&mut reader)
        .await
        .expect("counted");

    another_workspace(store, "Made meanwhile").await;

    let still: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspaces")
        .fetch_one(&mut reader)
        .await
        .expect("counted");
    assert_eq!(still, before, "the read sees what it began with");
    run(&mut reader, "COMMIT").await;
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspaces")
        .fetch_one(&mut reader)
        .await
        .expect("counted");
    assert_eq!(after, before + 1);
    reader.close().await.expect("the connection closes");
    store.close().await;
    scene.database.remove().await;
}

/// The file says how it is kept, and a store closed and opened again has what was
/// committed.
#[tokio::test]
async fn what_was_committed_is_there_when_the_file_is_opened_again() {
    let Some(scene) = on_a_file().await else {
        return;
    };
    let (store, path) = (&scene.store, &scene.path);
    let made = another_workspace(store, "Kept").await;
    store.close().await;

    assert_eq!(
        journal_of(path).await,
        "wal",
        "the mode is the file's, whoever opens it"
    );

    let again = scene.open(path).await;
    let kept = again.get_workspace(&made).await.expect("still there");
    assert_eq!(kept.title, "Kept");
    again.close().await;
    scene.database.remove().await;
}

/// A file an older server kept with the rollback journal is changed when it is opened,
/// with everything in it.
#[tokio::test]
async fn a_file_kept_with_the_rollback_journal_is_changed_when_it_is_opened() {
    let Some(scene) = on_a_file().await else {
        return;
    };
    let (store, path) = (&scene.store, &scene.path);
    let made = another_workspace(store, "From before").await;
    store.close().await;
    let mut older = outside(path).await;
    run(&mut older, "PRAGMA journal_mode = DELETE").await;
    older.close().await.expect("the connection closes");
    assert_eq!(
        journal_of(path).await,
        "delete",
        "the file as an older server left it"
    );

    let newer = scene.open(path).await;
    let kept = newer.get_workspace(&made).await.expect("still there");
    assert_eq!(kept.title, "From before");
    newer.close().await;
    assert_eq!(journal_of(path).await, "wal");
    scene.database.remove().await;
}

/// A server that was not closed (killed, or its machine): the file and what is beside
/// it, as they are on the disk at that moment, open with every commit. The last one is
/// in the log alone (a read under way keeps it from being moved into the file, as any
/// reader of a busy server does), so the file without its log is behind.
#[tokio::test]
async fn a_store_that_was_not_closed_leaves_every_commit_to_the_next() {
    let Some(scene) = on_a_file().await else {
        return;
    };
    let (store, path) = (&scene.store, &scene.path);
    let mut reading = outside(path).await;
    run(&mut reading, "BEGIN").await;
    let before = workspaces_named(&mut reading, "Committed last").await;
    assert_eq!(before, Ok(0));
    let made = another_workspace(store, "Committed last").await;

    // The disk as a server that ends here leaves it: the file with its log. The index
    // of the log (`-shm`) is of the memory of the machine, and is made anew.
    let left = beside(path, ".left");
    let alone = beside(path, ".alone");
    std::fs::copy(path, &left).expect("the file is copied");
    std::fs::copy(beside(path, "-wal"), beside(&left, "-wal")).expect("the log is copied");
    std::fs::copy(path, &alone).expect("the file is copied");
    run(&mut reading, "COMMIT").await;
    reading.close().await.expect("the connection closes");

    let mut behind = outside(&alone).await;
    let without_the_log = workspaces_named(&mut behind, "Committed last").await;
    behind.close().await.expect("the connection closes");
    // A file that never took anything out of its log has no tables at all.
    let behind = match &without_the_log {
        Ok(found) => *found == 0,
        Err(error) => error.contains("no such table"),
    };
    assert!(
        behind,
        "the last commit is in the log, not in the file: {without_the_log:?}"
    );
    let next = scene.open(&left).await;
    let kept = next.get_workspace(&made).await.expect("still there");
    assert_eq!(kept.title, "Committed last");
    next.close().await;
    store.close().await;
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(beside(&left, suffix));
        let _ = std::fs::remove_file(beside(&alone, suffix));
    }
    scene.database.remove().await;
}

/// A backup while the server works is taken through SQLite (`VACUUM INTO`, or the
/// `.backup` of its shell): one file, of one moment, with the last commit. (So it was
/// with the rollback journal; the test above shows what the file copied alone lacks.)
#[tokio::test]
async fn a_copy_taken_through_sqlite_while_the_store_works_has_the_last_commit() {
    let Some(scene) = on_a_file().await else {
        return;
    };
    let (store, path) = (&scene.store, &scene.path);
    let made = another_workspace(store, "Before the copy").await;

    let copy = beside(path, ".copy");
    let mut copying = outside(path).await;
    sqlx::query("VACUUM INTO ?")
        .bind(copy.display().to_string())
        .execute(&mut copying)
        .await
        .expect("the copy is taken");
    copying.close().await.expect("the connection closes");
    assert!(
        !beside(&copy, "-wal").exists(),
        "the copy is one file, with nothing beside it"
    );
    // Written after the copy: no part of it.
    let later = another_workspace(store, "After the copy").await;

    let restored = scene.open(&copy).await;
    let kept = restored.get_workspace(&made).await.expect("in the copy");
    assert_eq!(kept.title, "Before the copy");
    assert!(
        restored
            .get_workspace(&later)
            .await
            .is_err_and(|error| error.is_not_found()),
        "the copy is of the moment it was taken"
    );
    restored.close().await;
    store.close().await;
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(beside(&copy, suffix));
    }
    scene.database.remove().await;
}

/// Concurrent single-statement `with_write_conn` writes and multi-statement transactions
/// serialize through `sqlite_writers` in-process, succeeding even with `busy_timeout_ms=0`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_single_statement_writes_and_transactions_serialize_without_busy() {
    let Some(scene) = on_a_file().await else {
        return;
    };
    let path = scene.path.clone();
    let org_id = scene.org_id.clone();
    let ws_id = scene.workspace_id.clone();
    scene.store.close().await;

    let zero_timeout = Store::connect(
        DbType::Sqlite,
        &format!("{}?busy_timeout_ms=0", path.display()),
    )
    .await
    .expect("zero busy_timeout store opens")
    .for_org(&org_id)
    .expect("scoped");
    let user_id = crate::common::user(&zero_timeout, "writer").await;
    crate::common::works_in(&zero_timeout, &ws_id, &user_id).await;

    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(20));
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..20_i64 {
        let store = zero_timeout.clone();
        let b = std::sync::Arc::clone(&barrier);
        let uid = user_id.clone();
        let wid = ws_id.clone();
        tasks.spawn(async move {
            b.wait().await;
            if i % 3 == 0 {
                store
                    .record_welcome(&uid, 100 + i)
                    .await
                    .expect("record_welcome serialized");
            } else if i % 3 == 1 {
                store
                    .create_workspace(&Workspace {
                        id: new_id(),
                        title: format!("WS {i}"),
                        ..Workspace::default()
                    })
                    .await
                    .expect("create_workspace serialized");
            } else {
                let mut board = fvoci_model::Container {
                    id: new_id(),
                    workspace_id: wid,
                    title: format!("Board {i}"),
                    ..fvoci_model::Container::default()
                };
                store
                    .insert_container_with_admin(&mut board, &uid)
                    .await
                    .expect("transaction serialized");
            }
        });
    }
    for res in tasks.join_all().await {
        let () = res;
    }

    zero_timeout.close().await;
    scene.database.remove().await;
}
