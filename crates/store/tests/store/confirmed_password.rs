//! What a session confirmed of its account's password: recorded by one statement,
//! with the password still the one that was checked, and gone with the session.

use std::path::Path;

use fvoci_model::{Session, User, new_id};
use fvoci_store::{ConfirmedPassword, Store, StoreError};

use crate::common;
use crate::tenancy_sources::rust_sources;

/// A time in milliseconds that the tests count from.
const T0: i64 = 1_800_000_000_000;

const HASH: &str = "a-hash";

async fn user(test: &common::TestStore, hash: &str) -> User {
    let mut user = User {
        id: new_id(),
        username: new_id(),
        password_hash: hash.to_owned(),
        ..User::default()
    };
    test.create_user(&mut user).await.expect("user created");
    user
}

/// A session of the account, stored under the digest while its hash is `hash`.
async fn open(store: &Store, user_id: &str, digest: &str, hash: &str) -> Session {
    let session = Session {
        id: new_id(),
        user_id: user_id.to_owned(),
        created_at: T0,
        last_used_at: T0,
        expires_at: T0 + 100_000,
        user_agent: "a browser".to_owned(),
        // Stored as one that confirmed nothing, whatever is handed in.
        password_confirmed_at: Some(T0),
        ..Session::default()
    };
    let stored = store.create_session(&session, digest, hash).await;
    assert!(stored.expect("asked"), "{digest}: the session is stored");
    session
}

/// When the session under the digest confirmed the password; `None` too for a
/// session that is not there.
async fn confirmed_at(store: &Store, digest: &str) -> Option<i64> {
    match store.get_session(digest, T0 + 1, T0 - 1).await {
        Ok(session) => session.password_confirmed_at,
        Err(StoreError::NoRows) => None,
        Err(other) => panic!("looking a session up failed: {other:?}"),
    }
}

async fn is_there(store: &Store, digest: &str) -> bool {
    store.get_session(digest, T0 + 1, T0 - 1).await.is_ok()
}

/// The session of the account, as one that gave the password checked against `seen`.
fn of<'a>(session: &'a Session, user: &'a User, seen: &'a str) -> ConfirmedPassword<'a> {
    ConfirmedPassword {
        session_id: &session.id,
        user_id: &user.id,
        seen_password_hash: seen,
    }
}

/// A session is stored as one that confirmed nothing. Confirming records when, puts
/// the session under its new token, and leaves every other session as it was. It is
/// refused, and nothing is changed, unless the hash is still the one seen and is one,
/// the session is the account's and there, and the account is in use.
#[tokio::test]
async fn a_session_confirms_the_password_that_is_still_its_accounts() {
    let test = common::setup().await;
    let store = &test.store;
    let alice = user(&test, HASH).await;
    let bob = user(&test, HASH).await;
    let without = user(&test, "").await;
    let here = open(store, &alice.id, "here", HASH).await;
    let phone = open(store, &alice.id, "phone", HASH).await;
    let bobs = open(store, &bob.id, "bob", HASH).await;
    let free = open(store, &without.id, "free", "").await;
    assert_eq!(confirmed_at(store, "here").await, None, "stored without it");

    let gone = Session {
        id: new_id(),
        ..here.clone()
    };
    let refused = [
        (
            "a hash that is the account's no more",
            of(&here, &alice, "an-older-hash"),
            "x1",
        ),
        ("another account's session", of(&bobs, &alice, HASH), "x2"),
        (
            "an account without a password",
            of(&free, &without, ""),
            "x3",
        ),
        ("a session that is not there", of(&gone, &alice, HASH), "x4"),
    ];
    for (case, confirmed, digest) in refused {
        let recorded = store
            .confirm_session_password(confirmed, digest, T0 + 5)
            .await;
        assert!(!recorded.expect("asked"), "{case}");
        assert!(
            !is_there(store, digest).await,
            "{case}: no session is moved"
        );
    }
    for digest in ["here", "phone", "bob", "free"] {
        assert!(is_there(store, digest).await, "{digest} is where it was");
        assert_eq!(confirmed_at(store, digest).await, None, "{digest}");
    }

    let recorded = store
        .confirm_session_password(of(&here, &alice, HASH), "here-anew", T0 + 5)
        .await;
    assert!(recorded.expect("asked"), "the hash is the one seen");
    assert_eq!(confirmed_at(store, "here-anew").await, Some(T0 + 5));
    assert!(
        !is_there(store, "here").await,
        "its old token finds nothing"
    );
    assert_eq!(
        confirmed_at(store, "phone").await,
        None,
        "that session alone"
    );
    assert!(is_there(store, "phone").await && is_there(store, "bob").await);
    let _ = phone;

    // An account that waits to be deleted signs in to the way back and nothing else.
    let asked = store
        .request_account_deletion(&bob.id, HASH, T0, T0 + 1000)
        .await;
    asked.expect("the deletion is asked");
    let waiting = open(store, &bob.id, "bob-back", HASH).await;
    let recorded = store
        .confirm_session_password(of(&waiting, &bob, HASH), "bob-anew", T0 + 6)
        .await;
    assert!(!recorded.expect("asked"), "an account that waits");
    test.teardown().await;
}

/// How many sessions of the account are stored as having confirmed the password.
async fn confirmed_of(test: &common::TestStore, user_id: &str) -> usize {
    let query = format!(
        "SELECT CAST(id AS TEXT) FROM sessions WHERE user_id = '{user_id}' \
         AND password_confirmed_at <> 0"
    );
    let confirmed = test.system().texts_unchecked(&query).await;
    confirmed.expect("read").len()
}

/// Every change of the password ends the sessions that confirmed the one before, and
/// what they confirmed with them: the change by the account keeps the session it came
/// through, with what that confirmed; the reset by a link and the operator's keep
/// none.
#[tokio::test]
async fn a_change_of_the_password_leaves_nothing_confirmed_of_the_one_before() {
    let test = common::setup().await;
    let store = &test.store;
    let alice = user(&test, HASH).await;
    let confirm = async |digest: &str, hash: &'static str| {
        let session = open(store, &alice.id, digest, hash).await;
        let confirmed = ConfirmedPassword {
            session_id: &session.id,
            user_id: &alice.id,
            seen_password_hash: hash,
        };
        let anew = format!("{digest}-anew");
        let recorded = store
            .confirm_session_password(confirmed, &anew, T0 + 5)
            .await;
        assert!(recorded.expect("asked"), "{digest} confirms");
        session
    };
    let kept = confirm("kept", HASH).await;
    confirm("other", HASH).await;
    assert_eq!(confirmed_of(&test, &alice.id).await, 2);

    let changed = store
        .change_user_password(&alice.id, HASH, "second", &kept.id, "kept-third")
        .await;
    assert!(changed.expect("asked").is_some(), "the password is changed");
    assert_eq!(confirmed_at(store, "kept-third").await, Some(T0 + 5));
    assert!(
        !is_there(store, "other-anew").await,
        "the other session ended"
    );
    assert_eq!(confirmed_of(&test, &alice.id).await, 1);

    confirm("before-the-link", "second").await;
    let made = store
        .create_password_reset(&alice.id, &alice.email, "reset", T0)
        .await;
    assert!(made.expect("asked"), "a reset is stored");
    let consumed = store.consume_password_reset("reset", "third", T0 + 1).await;
    assert!(consumed.expect("asked").is_some(), "the reset is consumed");
    assert_eq!(
        confirmed_of(&test, &alice.id).await,
        0,
        "the link keeps none"
    );

    confirm("before-the-operator", "third").await;
    let reset = store.reset_user_password(&alice.username, "fourth").await;
    let _ = reset.expect("the operator sets the password");
    assert_eq!(confirmed_of(&test, &alice.id).await, 0, "nor the operator");
    test.teardown().await;
}

/// Whether a line of code names the column in the text of a statement: anywhere but
/// as the field of a session (`session.password_confirmed_at`,
/// `password_confirmed_at: ...`), whether the text starts with it, has it after
/// `UPDATE sessions SET`, or goes on from the line before.
fn names_column(line: &str) -> bool {
    line.replace(".password_confirmed_at", "")
        .replace("password_confirmed_at:", "")
        .contains("password_confirmed_at")
}

/// One statement sets what a session confirmed: the column is named by one constant
/// of one file, which is read wherever a session is and written by one `UPDATE`. A
/// session is stored without it. A file whose tests are a file of their own
/// (`mod tests;`) is read to its end.
#[test]
fn what_a_session_confirmed_is_set_by_one_statement() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crates directory");
    let mut files = Vec::new();
    for source in ["store/src", "server/src"] {
        rust_sources(&crates.join(source), &mut files);
    }
    assert!(files.len() > 50, "the sources are found: {}", files.len());
    for (line, named) in [
        ("\"UPDATE sessions SET password_confirmed_at = 1\"", true),
        ("SET password_confirmed_at = \"", true),
        ("\"password_confirmed_at\"", true),
        ("session.password_confirmed_at.is_some()", false),
        ("password_confirmed_at: Some(1),", false),
    ] {
        assert_eq!(names_column(line), named, "{line}");
    }
    let mut naming = Vec::new();
    let mut uses = Vec::new();
    for file in files {
        let name = file
            .strip_prefix(crates)
            .expect("under the crates directory")
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(&file).expect("a source file");
        let code = text
            .lines()
            .take_while(|line| !line.trim_start().starts_with("mod tests {"))
            .map(str::trim)
            .filter(|line| !line.starts_with("//"));
        for line in code {
            // In the text of a statement: by its name, or by the constant that is it.
            if names_column(line) || line.contains("CONFIRMED_COLUMN") {
                naming.push(name.clone());
            }
            if line.contains("CONFIRMED_COLUMN") && !line.starts_with("const CONFIRMED_COLUMN") {
                uses.push(line.to_owned());
            }
        }
    }
    naming.dedup();
    assert_eq!(
        naming,
        ["store/src/sessions.rs"],
        "one file names the column"
    );
    assert_eq!(
        uses,
        [
            "let mut sql = scope.sql([\"UPDATE sessions SET \", CONFIRMED_COLUMN, \" = \"]);",
            "CONFIRMED_COLUMN,",
            "CONFIRMED_COLUMN,",
        ],
        "read with a session, and set by one statement"
    );
}
