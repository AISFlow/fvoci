//! A gate that stops a transaction of the store where it is about to change a row, with
//! the row and all it held before locked, until the test opens it: two transactions
//! meet where a test wants them to, and nothing waits on the clock but the end of a
//! test that would otherwise hang. PostgreSQL only.

use std::time::Duration;

use fvoci_store::{DbType, Store};
use sqlx::PgConnection;
use tokio::task::JoinHandle;

use crate::common::{self, TestStore};

/// How long a test waits for a transaction to reach where it is to be: far longer than
/// it takes, so that only a transaction that never gets there runs out of it.
const PATIENCE: Duration = Duration::from_secs(20);

/// The name the connections of [`Gate::second_store`] go by, with the gate's key.
const SECOND: &str = "fvoci-test-second";

/// A closed gate: a trigger of the test's schema that waits for an advisory lock the
/// gate's own connection holds, or rows that connection holds in a transaction.
pub(crate) struct Gate {
    conn: PgConnection,
    /// The backend of the gate's connection, and the key of its advisory lock.
    key: i64,
    closed_by: ClosedBy,
}

/// What a gate holds closed.
#[derive(Debug, Clone, Copy)]
enum ClosedBy {
    /// Its advisory lock, which its trigger waits for.
    Trigger,
    /// Rows, in a transaction of its own.
    Rows,
}

impl Gate {
    /// Closes a gate before each `event` (`INSERT`, `UPDATE` or `DELETE`) of the rows of
    /// `table` that `when` picks (a trigger's condition, of `OLD` and `NEW`). The
    /// transaction stops holding what it held before, and for an update or a deletion
    /// the row too: a row trigger runs once its row is locked.
    pub(crate) async fn before(test: &TestStore, event: &str, table: &str, when: &str) -> Self {
        let mut conn = common::connect_past_the_store(&test.database_connection()).await;
        let key: i64 = sqlx::query_scalar("SELECT pg_backend_pid()::bigint")
            .fetch_one(&mut conn)
            .await
            .expect("the gate's connection has an identifier");
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(key)
            .execute(&mut conn)
            .await
            .expect("the gate closes");
        let trigger = format!(
            "CREATE FUNCTION test_gate() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN PERFORM pg_advisory_xact_lock({key}::bigint); \
             RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END; END $$; \
             CREATE TRIGGER test_gate BEFORE {event} ON {table} FOR EACH ROW \
             WHEN ({when}) EXECUTE FUNCTION test_gate()"
        );
        sqlx::raw_sql(sqlx::AssertSqlSafe(trigger))
            .execute(&mut conn)
            .await
            .expect("the gate is installed");
        Self {
            conn,
            key,
            closed_by: ClosedBy::Trigger,
        }
    }

    /// Closes a gate by holding, in a transaction of the gate's own, the rows `holding`
    /// locks (a `SELECT ... FOR UPDATE`): a transaction stops where it waits for them,
    /// holding what it held before.
    pub(crate) async fn holding(test: &TestStore, holding: &str) -> Self {
        let mut conn = common::connect_past_the_store(&test.database_connection()).await;
        let key: i64 = sqlx::query_scalar("SELECT pg_backend_pid()::bigint")
            .fetch_one(&mut conn)
            .await
            .expect("the gate's connection has an identifier");
        let held = format!("BEGIN; {holding}");
        sqlx::raw_sql(sqlx::AssertSqlSafe(held))
            .execute(&mut conn)
            .await
            .expect("the gate holds its rows");
        Self {
            conn,
            key,
            closed_by: ClosedBy::Rows,
        }
    }

    /// The backend of the transaction that stopped at the gate, once one has.
    pub(crate) async fn stopped<T>(&mut self, task: &JoinHandle<T>) -> i32 {
        let key = self.key;
        let conn = &mut self.conn;
        tokio::time::timeout(PATIENCE, async {
            loop {
                assert!(!task.is_finished(), "the transaction ended before the gate");
                let waiting: Option<i32> = sqlx::query_scalar(
                    "SELECT pid FROM pg_stat_activity \
                     WHERE $1 = ANY(pg_blocking_pids(pid)) LIMIT 1",
                )
                .bind(i32::try_from(key).expect("a backend's identifier"))
                .fetch_optional(&mut *conn)
                .await
                .expect("the gate's waiters are read");
                if let Some(pid) = waiting {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a transaction reaches the gate")
    }

    /// A store of the test's organization whose connections are told apart from all
    /// others: the one the second transaction runs on, which [`Gate::waits_for`] finds.
    /// Other tests' transactions may wait for the first one too, for a lock that is the
    /// server's and not a schema's.
    pub(crate) async fn second_store(&self, test: &TestStore) -> Store {
        self.second_store_of(test, &test.org_id).await
    }

    /// A store as [`Gate::second_store`] is, of the organization `org_id`.
    pub(crate) async fn second_store_of(&self, test: &TestStore, org_id: &str) -> Store {
        let database_connection = test.database_connection();
        let separator = if database_connection.contains('?') {
            '&'
        } else {
            '?'
        };
        let named = format!(
            "{database_connection}{separator}application_name={SECOND}-{}",
            self.key
        );
        Store::connect(DbType::Postgres, &named)
            .await
            .expect("a second store connects")
            .for_org(org_id)
            .expect("an identifier")
    }

    /// Waits until the transaction of `task`, on [`Gate::second_store`], waits for the
    /// backend `holder`, for something it holds; the task must not end first.
    pub(crate) async fn waits_for<T>(&mut self, holder: i32, task: &JoinHandle<T>) {
        let name = format!("{SECOND}-{}", self.key);
        let conn = &mut self.conn;
        tokio::time::timeout(PATIENCE, async {
            loop {
                assert!(
                    !task.is_finished(),
                    "the second transaction ended without waiting for the first"
                );
                let waits: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM pg_stat_activity \
                     WHERE application_name = $2 AND $1 = ANY(pg_blocking_pids(pid)))",
                )
                .bind(holder)
                .bind(&name)
                .fetch_one(&mut *conn)
                .await
                .expect("who waits is read");
                if waits {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the second transaction waits for the first");
    }

    /// Opens the gate, and lets every transaction through it from then on.
    pub(crate) async fn open(mut self) {
        let opening = match self.closed_by {
            ClosedBy::Trigger => sqlx::query("SELECT pg_advisory_unlock($1)").bind(self.key),
            ClosedBy::Rows => sqlx::query("ROLLBACK"),
        };
        opening
            .execute(&mut self.conn)
            .await
            .expect("the gate opens");
    }
}

/// What a task the test started came to, which it waits for no longer than a test may.
pub(crate) async fn ended<T>(task: JoinHandle<T>) -> T {
    tokio::time::timeout(PATIENCE, task)
        .await
        .expect("the transaction ends")
        .expect("the task does not panic")
}
