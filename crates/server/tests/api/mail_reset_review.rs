//! Review of password recovery: what a reset leaves of the limits on signing in.

use std::net::{IpAddr, Ipv4Addr};

use reqwest::StatusCode;
use serde_json::json;

use crate::common::{PASSWORD, Setup};

/// Who proves the mailbox by a reset link chose the only password that now opens the
/// account, yet the lock that others' wrong guesses put on the old one still refuses it.
#[tokio::test]
async fn a_reset_password_signs_in_although_others_locked_the_account() {
    let th = Setup::native()
        .configure(|config| config.login_account_max_failures = 2)
        .start()
        .await
        .init_basic()
        .await;
    for _ in 0..2 {
        th.anonymous()
            .login("user1", "not-the-password")
            .await
            .check_status(StatusCode::UNAUTHORIZED);
    }
    th.anonymous()
        .login("user1", PASSWORD)
        .await
        .check_status(StatusCode::TOO_MANY_REQUESTS);

    let url = th
        .app
        .password_reset_link("user1")
        .await
        .expect("link issued");
    let token = url.split_once('#').expect("a fragment").1;
    th.anonymous()
        .post(
            "/password-reset/complete",
            &json!({"token": token, "password": "New-password-1"}),
        )
        .await
        .check_no_content();

    th.anonymous()
        .login("user1", "New-password-1")
        .await
        .check_no_content();
    th.teardown().await;
}

/// The lock that guesses at the account's email left ends as well, and a guesser at
/// another address still waits for what it failed there.
#[tokio::test]
async fn a_reset_ends_the_lock_at_the_email_but_not_a_guessers_wait() {
    let th = Setup::native()
        .configure(|config| {
            config.login_account_max_failures = 2;
            config.login_pair_free_failures = 1;
        })
        .start()
        .await
        .init_basic()
        .await;
    let guesser = Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9)));
    for _ in 0..2 {
        assert!(
            th.app
                .login("", "USER1@sample.com", "not-the-password", "", guesser)
                .await
                .is_err()
        );
    }
    let by_email = json!({"email": "user1@sample.com", "password": PASSWORD});
    th.anonymous()
        .post("/login", &by_email)
        .await
        .check_status(StatusCode::TOO_MANY_REQUESTS);

    let url = th
        .app
        .password_reset_link("user1")
        .await
        .expect("link issued");
    let token = url.split_once('#').expect("a fragment").1;
    th.anonymous()
        .post(
            "/password-reset/complete",
            &json!({"token": token, "password": "New-password-1"}),
        )
        .await
        .check_no_content();
    th.anonymous()
        .post(
            "/login",
            &json!({"email": "User1@sample.com", "password": "New-password-1"}),
        )
        .await
        .check_no_content();
    assert!(
        th.app
            .login("", "user1@sample.com", "New-password-1", "", guesser)
            .await
            .is_err(),
        "the guesser's address still waits"
    );
    th.teardown().await;
}
