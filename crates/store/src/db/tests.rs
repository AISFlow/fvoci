use super::is_plain_select;

#[test]
fn only_a_statement_that_begins_with_select_is_taken_for_a_read() {
    assert!(
        is_plain_select("SELECT id FROM containers WHERE id = $1"),
        "a read"
    );
    assert!(is_plain_select("  SELECT 1"), "leading white space");
    for statement in [
        "WITH gone AS (DELETE FROM blocks RETURNING id) SELECT id FROM gone",
        "DELETE FROM data_steps WHERE name = $1 RETURNING name",
        "UPDATE users SET updated_at = $1 RETURNING id",
        "INSERT INTO containers (id) VALUES ($1) RETURNING id",
        "select 1",
        "SELECTED",
    ] {
        assert!(!is_plain_select(statement), "{statement}");
    }
}

#[tokio::test]
async fn a_statement_that_runs_past_its_time_is_stopped_and_the_connection_serves_on() {
    use std::time::{Duration, Instant};

    use super::Dialect;
    use crate::Store;
    use crate::testing::TestDatabase;

    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/deadline-tests");
    let database = TestDatabase::create(&directory).await;
    let store = Store::connect(database.db_type(), database.database_connection())
        .await
        .expect("test store");
    let counting = |to: u64| {
        format!(
            "SELECT count(*) FROM (WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL \
             SELECT x + 1 FROM n WHERE x < {to}) SELECT x FROM n) AS counted"
        )
    };
    let endless = match store.dialect() {
        Dialect::Sqlite => counting(u64::MAX / 2),
        Dialect::Postgres => "SELECT pg_sleep(60)".to_owned(),
    };
    let run = async |text: &str, by: Instant| {
        store
            .with_conn(async |scope| {
                let sql = scope.sql_unchecked(text);
                scope.fetch_all_by(sql, by).await
            })
            .await
    };

    let started = Instant::now();
    let stopped = run(&endless, started + Duration::from_millis(300)).await;
    let took = started.elapsed();
    let late = run(&counting(10), started).await;
    // Long past the time of the statement that was stopped: its handler is gone.
    let mut after = Vec::new();
    for _ in 0..4 {
        after.push(run(&counting(300_000), Instant::now() + Duration::from_secs(30)).await);
    }
    let plain = store
        .with_conn(async |scope| {
            let sql = scope.sql_unchecked(&counting(300_000));
            scope.fetch_all(sql).await
        })
        .await;
    store.close().await;
    database.remove().await;

    assert!(matches!(stopped, Ok(None)), "{stopped:?}");
    assert!(took < Duration::from_secs(5), "stopped in time: {took:?}");
    assert!(
        matches!(late, Ok(None)),
        "nothing asked past the time: {late:?}"
    );
    for rows in after {
        assert!(
            matches!(rows, Ok(Some(ref rows)) if rows.len() == 1),
            "{rows:?}"
        );
    }
    assert!(plain.is_ok(), "{plain:?}");
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn a_lock_is_recorded_as_it_is_taken_and_its_clause_is_the_dialects() {
    use super::locks::TakenLock;
    use super::{Dialect, LockRank, RowLock};
    use crate::Store;
    use crate::testing::TestDatabase;

    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/lock-tests");
    let database = TestDatabase::create(&directory).await;
    let store = Store::connect(database.db_type(), database.database_connection())
        .await
        .expect("test store");
    let ran = store
        .with_conn(async |scope| {
            let clause = scope.row_lock(LockRank::Container, "c", RowLock::Update);
            let taken: Vec<_> = scope.locks.iter().map(TakenLock::parts).collect();
            assert_eq!(taken, [(LockRank::Container, "c", RowLock::Update)]);
            let mut sql = scope.sql("SELECT 1");
            sql.push_lock(clause);
            let written = match scope.dialect() {
                Dialect::Sqlite => "SELECT 1",
                Dialect::Postgres => "SELECT 1 FOR UPDATE",
            };
            assert_eq!(sql.text, written);
            if scope.dialect() == Dialect::Postgres {
                return Ok(());
            }
            // SQLite holds no row: a lock that only holds is recorded and not run.
            let not_run = scope.sql("SELECT * FROM no_such_table");
            scope
                .hold(LockRank::Block, "b", not_run, RowLock::Update)
                .await?;
            // A row found through what it is joined to is recorded by its first column,
            // once it is found.
            let nothing = scope.sql("SELECT 'w:x', 1 WHERE 1 = 0");
            let found: Option<(String, i64)> = scope
                .first_held(LockRank::WorkspaceMembers, nothing, RowLock::Share)
                .await?;
            assert_eq!(found, None);
            let one = scope.sql("SELECT 'w:u', 7");
            let found: Option<(String, i64)> = scope
                .first_held(LockRank::WorkspaceMembers, one, RowLock::Share)
                .await?;
            assert_eq!(found, Some(("w:u".to_owned(), 7)));
            let taken: Vec<_> = scope.locks.iter().map(TakenLock::parts).collect();
            assert_eq!(
                taken,
                [
                    (LockRank::Container, "c", RowLock::Update),
                    (LockRank::Block, "b", RowLock::Update),
                    (LockRank::WorkspaceMembers, "w:u", RowLock::Share),
                ]
            );
            Ok(())
        })
        .await;
    store.close().await;
    database.remove().await;
    assert!(ran.is_ok(), "{ran:?}");
}
