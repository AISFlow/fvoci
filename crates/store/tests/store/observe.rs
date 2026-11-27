//! What a store tells its observer about its calls.

use std::future::Future as _;
use std::sync::{Arc, Mutex};
use std::task::{Context, Waker};
use std::time::Duration;

use fvoci_model::{User, new_id};
use fvoci_store::{Access, Call, DbType, Observer, Outcome};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions as _, Connection as _};

use crate::common;

/// What an observer was told.
#[derive(Debug, Default)]
struct Told {
    calls: Vec<Call>,
    waits: Vec<(DbType, Duration)>,
}

/// An observer that keeps what it is told.
#[derive(Debug, Clone, Default)]
struct Recorder(Arc<Mutex<Told>>);

impl Recorder {
    /// The calls told since the last time, and how many waits.
    fn take(&self) -> (Vec<Call>, usize) {
        let mut told = self.0.lock().expect("lock poisoned");
        let calls = std::mem::take(&mut told.calls);
        let waits = std::mem::take(&mut told.waits).len();
        (calls, waits)
    }
}

impl Observer for Recorder {
    fn called(&self, call: Call) {
        self.0.lock().expect("lock poisoned").calls.push(call);
    }

    fn waited_for_connection(&self, db: DbType, waited: Duration) {
        self.0
            .lock()
            .expect("lock poisoned")
            .waits
            .push((db, waited));
    }
}

/// How many calls are begun and given up at once before one of them is still waiting.
/// Each try takes no time: on a fast machine an SQLite read often ends within its first
/// poll (about 19 in 20 here), and twenty tries left the test failing two runs in five.
const TRIES_TO_GIVE_UP: usize = 1_000;

/// The access and the outcome of each call.
fn shapes(calls: &[Call]) -> Vec<(Access, Outcome)> {
    calls
        .iter()
        .map(|call| (call.access, call.outcome))
        .collect()
}

#[tokio::test]
async fn every_call_is_told_with_whether_it_wrote_and_how_it_ended() {
    let store = common::setup().await;
    let recorder = Recorder::default();
    assert!(
        store.observe(Box::new(recorder.clone())),
        "the first observer"
    );
    assert!(
        !store.observe(Box::new(Recorder::default())),
        "a store has one observer"
    );
    let db = common::db_type();

    // A read that finds what it asks for, and one that does not.
    store
        .get_registered_user_count()
        .await
        .expect("the users are counted");
    let missing = store.get_user_by_id(&new_id()).await;
    assert!(missing.is_err(), "no such user");
    // A transaction that writes, and one the database refuses.
    let user_id = common::user(&store, "observed").await;
    let mut twin = User {
        id: new_id(),
        username: "observed".to_owned(),
        email: "twin@example.com".to_owned(),
        ..User::default()
    };
    assert!(
        store.create_user(&mut twin).await.is_err(),
        "the name is taken"
    );
    // One statement that writes, on one connection.
    let _ = store
        .delete_session(&new_id())
        .await
        .expect("no such session is deleted");
    store
        .get_user_by_id(&user_id)
        .await
        .expect("the user is there");
    let (calls, waits) = recorder.take();
    assert_eq!(
        shapes(&calls),
        [
            (Access::Read, Outcome::Done),
            (Access::Read, Outcome::Refused),
            (Access::Write, Outcome::Done),
            (Access::Write, Outcome::Refused),
            (Access::Write, Outcome::Done),
            (Access::Read, Outcome::Done),
        ],
        "{calls:?}"
    );
    assert!(calls.iter().all(|call| call.db == db), "{calls:?}");
    assert_eq!(waits, calls.len(), "every call asked for a connection once");

    // Another handle to the same database tells the same observer.
    store
        .system()
        .get_registered_user_count()
        .await
        .expect("the users are counted");
    assert_eq!(recorder.take().0.len(), 1, "the system handle's call");

    // A call its caller gives up is told as given up, when it is dropped.
    let mut abandoned = false;
    for _ in 0..TRIES_TO_GIVE_UP {
        // A zero-duration timer still waits for a timer tick. Drop a pending call
        // after its first poll, before a fast database can answer on the next poll.
        let given_up = {
            let mut call = std::pin::pin!(store.get_registered_user_count());
            call.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        };
        let (calls, _) = recorder.take();
        if given_up {
            assert_eq!(
                shapes(&calls),
                [(Access::Read, Outcome::Abandoned)],
                "{calls:?}"
            );
            abandoned = true;
            break;
        }
    }
    assert!(abandoned, "a call was given up before it ended");

    // A call that gets no connection fails, and its wait is told.
    store.close().await;
    assert!(store.get_registered_user_count().await.is_err(), "closed");
    let (calls, waits) = recorder.take();
    assert_eq!(
        shapes(&calls),
        [(Access::Read, Outcome::Failed)],
        "{calls:?}"
    );
    assert_eq!(waits, 1, "the wait that got nothing");
    store.teardown().await;
}

/// A call on one connection given up while its statement that writes waits for a lock
/// that another connection holds is told as one that wrote: it started writing, though
/// it never said so by ending.
#[tokio::test]
async fn a_cancelled_statement_that_started_writing_is_observed_as_a_write() {
    let store = common::setup().await;
    let recorder = Recorder::default();
    assert!(store.observe(Box::new(recorder.clone())), "the observer");
    let database_connection = store.database_connection();
    let session_id = new_id();
    let delete = || tokio::time::timeout(Duration::from_secs(1), store.delete_session(&session_id));
    let given_up = match common::db_type() {
        DbType::Sqlite => {
            let mut holder = SqliteConnectOptions::new()
                .filename(&database_connection)
                .connect()
                .await
                .expect("a connection of the test's own");
            let lock = holder
                .begin_with("BEGIN IMMEDIATE")
                .await
                .expect("the write lock is held");
            let given_up = delete().await.is_err();
            lock.rollback().await.expect("the write lock is let go");
            holder.close().await.expect("the connection closes");
            given_up
        }
        DbType::Postgres => {
            let mut holder = common::connect_past_the_store(&database_connection).await;
            let mut lock = holder
                .begin()
                .await
                .expect("a transaction of the test's own");
            sqlx::raw_sql("LOCK TABLE sessions IN ACCESS EXCLUSIVE MODE")
                .execute(&mut *lock)
                .await
                .expect("the table lock is held");
            let given_up = delete().await.is_err();
            lock.rollback().await.expect("the table lock is let go");
            holder.close().await.expect("the connection closes");
            given_up
        }
    };
    let (calls, waits) = recorder.take();
    store.teardown().await;
    assert!(
        given_up,
        "the write waits for the lock until it is given up"
    );
    assert_eq!(
        shapes(&calls),
        [(Access::Write, Outcome::Abandoned)],
        "told once, as a write given up: {calls:?}"
    );
    assert_eq!(waits, 1, "one wait for a connection");
}
