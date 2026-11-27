//! Reset credentials are single-use, time-limited and end all sessions atomically.

use fvoci_model::{Session, User, new_id};
use fvoci_store::PASSWORD_RESET_LIFETIME_MILLIS;

use crate::common;

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

#[tokio::test]
async fn a_reset_is_consumed_once_and_ends_all_sessions() {
    let test = common::setup().await;
    let account = user(&test, "old").await;
    for digest in ["session-one", "session-two"] {
        let session = Session {
            id: new_id(),
            user_id: account.id.clone(),
            created_at: 10,
            expires_at: 10000,
            ..Session::default()
        };
        assert!(
            test.create_session(&session, digest, "old")
                .await
                .expect("session created")
        );
    }
    assert!(
        test.create_password_reset(&account.id, &account.email, "digest", 100)
            .await
            .expect("reset stored")
    );
    let (one, two) = tokio::join!(
        test.consume_password_reset("digest", "new-one", 101),
        test.consume_password_reset("digest", "new-two", 101),
    );
    assert_eq!(
        usize::from(one.expect("first attempt").is_some())
            + usize::from(two.expect("second attempt").is_some()),
        1,
        "exactly one consumer succeeds"
    );
    for digest in ["session-one", "session-two"] {
        assert!(
            test.get_session(digest, 101, 0)
                .await
                .expect_err("ended session")
                .is_not_found()
        );
    }
    let hash = test
        .get_user_by_id(&account.id)
        .await
        .expect("user")
        .password_hash;
    assert!(
        hash == "new-one" || hash == "new-two",
        "winning password stored"
    );
    assert!(
        test.consume_password_reset("digest", "replay", 102)
            .await
            .expect("replay")
            .is_none()
    );
    assert_eq!(
        test.get_user_by_id(&account.id)
            .await
            .expect("user")
            .password_hash,
        hash
    );
    test.teardown().await;
}

#[tokio::test]
async fn expiry_is_exactly_one_hour_and_unknown_tokens_change_nothing() {
    let test = common::setup().await;
    let account = user(&test, "old").await;
    assert!(
        test.create_password_reset(&account.id, &account.email, "expired", 100)
            .await
            .expect("stored")
    );
    for (digest, now) in [
        ("expired", 100 + PASSWORD_RESET_LIFETIME_MILLIS),
        ("unknown", 101),
    ] {
        assert!(
            test.consume_password_reset(digest, "bad", now)
                .await
                .expect("refused")
                .is_none()
        );
        assert_eq!(
            test.get_user_by_id(&account.id)
                .await
                .expect("user")
                .password_hash,
            "old"
        );
    }
    assert!(
        test.create_password_reset(&account.id, &account.email, "live", 100)
            .await
            .expect("stored")
    );
    assert!(
        test.consume_password_reset("live", "new", 100 + PASSWORD_RESET_LIFETIME_MILLIS - 1000)
            .await
            .expect("live")
            .is_some()
    );
    test.teardown().await;
}

#[tokio::test]
async fn accounts_without_passwords_never_get_reset_credentials() {
    let test = common::setup().await;
    let account = user(&test, "").await;
    assert!(
        !test
            .create_password_reset(&account.id, &account.email, "digest", 100)
            .await
            .expect("not stored")
    );
    assert!(
        test.consume_password_reset("digest", "native", 101)
            .await
            .expect("refused")
            .is_none()
    );
    assert_eq!(
        test.get_user_by_id(&account.id)
            .await
            .expect("user")
            .password_hash,
        ""
    );
    test.teardown().await;
}

#[tokio::test]
async fn setting_a_password_any_way_revokes_every_earlier_reset_credential() {
    let test = common::setup().await;
    let account = user(&test, "old").await;
    for digest in ["used", "sibling"] {
        assert!(
            test.create_password_reset(&account.id, &account.email, digest, 100)
                .await
                .expect("reset stored")
        );
    }
    let ended = test
        .consume_password_reset("used", "reset", 101)
        .await
        .expect("consumed");
    assert_eq!(
        ended.as_ref().map(|set| set.user_id.as_str()),
        Some(account.id.as_str())
    );
    assert!(
        test.consume_password_reset("sibling", "again", 102)
            .await
            .expect("refused")
            .is_none(),
        "a link sent before the reset does not set the password again"
    );

    let kept = Session {
        id: new_id(),
        user_id: account.id.clone(),
        created_at: 10,
        expires_at: i64::MAX,
        ..Session::default()
    };
    assert!(
        test.create_session(&kept, "kept", "reset")
            .await
            .expect("session created")
    );
    assert!(
        test.create_password_reset(&account.id, &account.email, "before-change", 200)
            .await
            .expect("reset stored")
    );
    assert!(
        test.change_user_password(&account.id, "reset", "changed", &kept.id, "kept-anew")
            .await
            .expect("changed")
            .is_some()
    );
    assert!(
        test.consume_password_reset("before-change", "again", 201)
            .await
            .expect("refused")
            .is_none(),
        "changing the password revokes the links"
    );

    assert!(
        test.create_password_reset(&account.id, &account.email, "before-operator", 300)
            .await
            .expect("reset stored")
    );
    let _ = test
        .reset_user_password(&account.username, "operator")
        .await
        .expect("set by the operator");
    assert!(
        test.consume_password_reset("before-operator", "again", 301)
            .await
            .expect("refused")
            .is_none(),
        "a password the operator sets revokes the links"
    );
    assert_eq!(
        test.get_user_by_id(&account.id)
            .await
            .expect("user")
            .password_hash,
        "operator"
    );
    test.teardown().await;
}
