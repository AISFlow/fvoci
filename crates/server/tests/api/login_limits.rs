//! Limits on login attempts: an address that keeps failing at an account waits, an
//! account that keeps being failed at is locked, an address makes only so many attempts
//! a minute, and the answer in each case is a 429 that says how long to wait.

use fvoci_model::RegisterRequest;
use reqwest::{Method, StatusCode};
use serde_json::json;

use crate::common::{ApiResponse, Client, PASSWORD, Setup, TestServer, USER1_USERNAME};

fn credentials(username: &str, password: &str) -> serde_json::Value {
    json!({"username": username, "password": password})
}

async fn login(th: &TestServer, username: &str, password: &str) -> ApiResponse {
    th.anonymous()
        .post("/login", &credentials(username, password))
        .await
}

/// A login as it arrives through the reverse proxy the server trusts, from `address`.
async fn login_from(th: &TestServer, address: &str, username: &str, password: &str) -> ApiResponse {
    let request = th
        .anonymous()
        .request(Method::POST, "/login")
        .header("X-Forwarded-For", address)
        .json(&credentials(username, password));
    Client::send(request).await
}

/// Checks that the answer is the refusal of one attempt too many, and returns how many
/// seconds it says to wait: the same in the header and in the body.
#[track_caller]
fn retry_after(response: &ApiResponse) -> u64 {
    response.check_error(
        StatusCode::TOO_MANY_REQUESTS,
        "too many attempts; try again later",
    );
    let in_body = response.value()["retryAfter"]
        .as_u64()
        .expect("the body says how long to wait");
    assert_eq!(
        response.header("retry-after"),
        in_body.to_string(),
        "the Retry-After header says the same"
    );
    assert!(in_body >= 1, "at least a second");
    in_body
}

/// After the failures an address has for free at an account, each further one makes it
/// wait, twice as long each time. The wait is the same for a wrong password and for an
/// account that does not exist, and a right password does not get through it either.
#[tokio::test]
async fn an_address_that_keeps_failing_at_an_account_is_made_to_wait() {
    let th = Setup::native()
        .configure(|config| {
            config.login_pair_free_failures = 2;
            config.login_pair_base_delay_seconds = 30;
        })
        .start()
        .await
        .init_basic()
        .await;

    for account in [USER1_USERNAME, "nobody-by-this-name"] {
        for attempt in 0..3 {
            let response = login(&th, account, "a wrong password").await;
            response.check_error(StatusCode::UNAUTHORIZED, "incorrect login");
            assert_eq!(
                response.error_reason(),
                "invalid_credentials",
                "{account}, attempt {attempt}: the same refusal whether the account exists"
            );
        }
        // Two were free; the third started the clock.
        let wait = retry_after(&login(&th, account, "a wrong password").await);
        assert!(
            (29..=30).contains(&wait),
            "{account}: about 30 s, was {wait}"
        );
        if account == USER1_USERNAME {
            let wait = retry_after(&login(&th, account, PASSWORD).await);
            assert!(wait <= 30, "the right password waits too: {wait}");
        }
    }
    // The name in other letters is the same account; another account is not held up.
    retry_after(&login(&th, &USER1_USERNAME.to_uppercase(), PASSWORD).await);
    login(&th, "user2", PASSWORD).await.check_no_content();
    // Signed in before the trouble: the session goes on.
    th.client.get("/users/me").await.check_ok();

    th.teardown().await;
}

/// Who has to wait is decided by what the attempts said, never by what was found: an
/// attempt is counted at the names it gives before any of them is looked up. Otherwise
/// a second spelling of a name, which finds no account, would wait with the name when
/// there is no such account and not when there is one, and the wait would tell them
/// apart.
#[tokio::test]
async fn who_has_to_wait_does_not_tell_which_names_are_accounts() {
    let th = Setup::native()
        .configure(|config| {
            config.login_pair_free_failures = 2;
            config.login_pair_base_delay_seconds = 30;
        })
        .start()
        .await
        .init_basic()
        .await;
    let wrong = "a wrong password";

    // A spelling that finds no account either way: the name with a space after it.
    for name in [USER1_USERNAME, "nobody-by-this-name"] {
        for _ in 0..3 {
            login(&th, &format!("{name} "), wrong)
                .await
                .check_error(StatusCode::UNAUTHORIZED, "incorrect login");
        }
        let wait = retry_after(&login(&th, name, wrong).await);
        assert!((29..=30).contains(&wait), "{name}: about 30 s, was {wait}");
    }

    // An email address, tried beside names that are nobody's.
    for email in ["user2@sample.com", "nobody@sample.com"] {
        for attempt in 0..3 {
            let body = json!({
                "username": format!("no-such-user-{attempt}"),
                "email": email,
                "password": wrong,
            });
            th.anonymous()
                .post("/login", &body)
                .await
                .check_error(StatusCode::UNAUTHORIZED, "incorrect login");
        }
        let by_email = json!({"email": email.to_uppercase(), "password": wrong});
        let wait = retry_after(&th.anonymous().post("/login", &by_email).await);
        assert!((29..=30).contains(&wait), "{email}: about 30 s, was {wait}");
    }
    // The name and the email address of an account are counted each for itself: what
    // was failed at the address of user2 does not hold up user2 by name.
    login(&th, "user2", PASSWORD).await.check_no_content();

    th.teardown().await;
}

/// A session is not the password: whoever holds one gets no more guesses at the old
/// password through changing it than through signing in, and the guesses are the
/// same count.
#[tokio::test]
async fn the_old_password_is_not_guessed_at_through_a_session() {
    let th = Setup::native()
        .configure(|config| {
            config.login_pair_free_failures = 2;
            config.login_pair_base_delay_seconds = 30;
        })
        .start()
        .await
        .init_basic()
        .await;
    let change = |old: &'static str| {
        let client = &th.client;
        async move {
            client
                .post(
                    "/users/me/password",
                    &json!({"oldPassword": old, "newPassword": "another password"}),
                )
                .await
        }
    };

    for _ in 0..3 {
        let response = change("a wrong guess").await;
        response.check_error(StatusCode::BAD_REQUEST, "the old password is wrong");
        assert_eq!(response.error_reason(), "invalid_credentials");
    }
    let wait = retry_after(&change("a wrong guess").await);
    assert!((29..=30).contains(&wait), "about 30 s, was {wait}");
    retry_after(&change(PASSWORD).await);
    // The same count as signing in to the account by its name.
    retry_after(&login(&th, USER1_USERNAME, PASSWORD).await);
    // Nothing was changed, and the session goes on.
    th.client.get("/users/me").await.check_ok();

    th.teardown().await;
}

/// Nor through confirming it in a session: the guesses are counted with the logins to
/// the account by its name, wait as long, and are answered as a wrong old password is.
#[tokio::test]
async fn the_password_is_not_guessed_at_through_confirming_it() {
    let th = Setup::native()
        .configure(|config| {
            config.login_pair_free_failures = 2;
            config.login_pair_base_delay_seconds = 30;
        })
        .start()
        .await
        .init_basic()
        .await;
    let confirm = |password: &'static str| {
        let client = &th.client;
        async move {
            client
                .post("/users/me/password/confirm", &json!({"password": password}))
                .await
        }
    };

    for _ in 0..3 {
        let response = confirm("a wrong guess").await;
        response.check_error(StatusCode::BAD_REQUEST, "the password is wrong");
        assert_eq!(response.error_reason(), "invalid_credentials");
    }
    let wait = retry_after(&confirm("a wrong guess").await);
    assert!((29..=30).contains(&wait), "about 30 s, was {wait}");
    retry_after(&confirm(PASSWORD).await);
    // The same count as signing in to the account by its name.
    retry_after(&login(&th, USER1_USERNAME, PASSWORD).await);
    // Nothing was recorded, and the session goes on under the cookie it had.
    th.client.get("/users/me").await.check_ok();

    th.teardown().await;
}

/// Nor through asking to change the account's address: the guesses are counted with the
/// logins to the account by its name and wait as long, and nothing is asked for.
#[tokio::test]
async fn the_password_is_not_guessed_at_through_changing_the_address() {
    let th = Setup::native()
        .configure(|config| {
            config.login_pair_free_failures = 2;
            config.login_pair_base_delay_seconds = 30;
        })
        .start()
        .await
        .init_basic()
        .await;
    let ask = |password: &'static str| {
        let client = &th.client;
        async move {
            client
                .post(
                    "/users/me/email-change",
                    &json!({"email": "moved@example.test", "password": password}),
                )
                .await
        }
    };

    for _ in 0..3 {
        let response = ask("a wrong guess").await;
        response.check_error(StatusCode::BAD_REQUEST, "the password is wrong");
        assert_eq!(response.error_reason(), "invalid_credentials");
    }
    let wait = retry_after(&ask("a wrong guess").await);
    assert!((29..=30).contains(&wait), "about 30 s, was {wait}");
    retry_after(&ask(PASSWORD).await);
    // The same count as signing in to the account by its name.
    retry_after(&login(&th, USER1_USERNAME, PASSWORD).await);
    // The address is as it was, and the session goes on.
    let me: fvoci_model::Me = th.client.get("/users/me").await.ok_json();
    assert_eq!(
        (me.email.as_str(), me.pending_email),
        ("user1@sample.com", None)
    );

    th.teardown().await;
}

/// Only who fails waits: the same account from another address is not held up, and a
/// success ends the count of the address that succeeded, not of the one that guesses.
/// Addresses are told apart behind a trusted proxy by what the proxy says; the header
/// of somebody who is no trusted proxy is not believed.
#[tokio::test]
async fn only_the_address_that_fails_waits() {
    let th = Setup::native()
        .configure(|config| {
            config.login_pair_free_failures = 1;
            config.trusted_proxies = vec!["127.0.0.1".to_owned()];
        })
        .start()
        .await
        .init_basic()
        .await;
    let guesser = "203.0.113.7";
    let owner = "198.51.100.1";

    for _ in 0..2 {
        login_from(&th, guesser, USER1_USERNAME, "a guess")
            .await
            .check_unauthorized();
    }
    retry_after(&login_from(&th, guesser, USER1_USERNAME, "a guess").await);

    login_from(&th, owner, USER1_USERNAME, PASSWORD)
        .await
        .check_no_content();
    retry_after(&login_from(&th, guesser, USER1_USERNAME, PASSWORD).await);
    // What the client writes at the left of the header is not what the proxy saw.
    retry_after(
        &login_from(
            &th,
            &format!("{owner}, {guesser}"),
            USER1_USERNAME,
            PASSWORD,
        )
        .await,
    );
    login_from(
        &th,
        &format!("{guesser}, {owner}"),
        USER1_USERNAME,
        PASSWORD,
    )
    .await
    .check_no_content();

    th.teardown().await;
}

/// Without a trusted proxy the header is nobody's to set: all attempts come from the
/// address of the connection, whatever they claim.
#[tokio::test]
async fn the_forwarded_address_is_believed_from_trusted_proxies_only() {
    let th = Setup::native()
        .configure(|config| config.login_pair_free_failures = 1)
        .start()
        .await
        .init_basic()
        .await;

    for claimed in ["203.0.113.1", "203.0.113.2"] {
        login_from(&th, claimed, USER1_USERNAME, "a guess")
            .await
            .check_unauthorized();
    }
    retry_after(&login_from(&th, "203.0.113.3", USER1_USERNAME, "a guess").await);

    th.teardown().await;
}

/// Failures in a row from anywhere lock the account for a while, for everybody: that
/// is what stops guessing from many addresses. Other accounts are not touched.
#[tokio::test]
async fn an_account_is_locked_after_too_many_failures_from_anywhere() {
    let th = Setup::native()
        .configure(|config| {
            config.login_account_max_failures = 4;
            config.login_account_lock_seconds = 600;
            config.trusted_proxies = vec!["127.0.0.1".to_owned()];
        })
        .start()
        .await
        .init_basic()
        .await;

    // Four addresses, one failure each: none of them has anything to wait for.
    for host in 1..=4 {
        login_from(&th, &format!("203.0.113.{host}"), USER1_USERNAME, "a guess")
            .await
            .check_unauthorized();
    }

    let wait = retry_after(&login_from(&th, "198.51.100.1", USER1_USERNAME, PASSWORD).await);
    assert!(
        (599..=600).contains(&wait),
        "locked for ten minutes: {wait}"
    );
    login_from(&th, "198.51.100.1", "user2", PASSWORD)
        .await
        .check_no_content();

    th.teardown().await;
}

/// An address makes only so many attempts a minute, successes and registrations
/// included, whatever the accounts: that is what stops one password tried on many
/// accounts.
#[tokio::test]
async fn an_address_makes_only_so_many_attempts_a_minute() {
    let th = Setup::native()
        .configure(|config| config.login_address_attempts_per_minute = 4)
        .start()
        .await;
    let register = |username: &str| RegisterRequest {
        username: username.to_owned(),
        email: format!("{username}@sample.com"),
        password: PASSWORD.to_owned(),
        display_name: String::new(),
        organization_name: "The Firm".to_owned(),
        language: String::new(),
    };

    // A registration, a success, and two failures at accounts that do not exist.
    th.anonymous()
        .post("/register", &register("first"))
        .await
        .check_created();
    login(&th, "first", PASSWORD).await.check_no_content();
    login(&th, "one", "password1").await.check_unauthorized();
    login(&th, "two", "password1").await.check_unauthorized();

    let wait = retry_after(&login(&th, "three", "password1").await);
    assert!(wait <= 60, "room again within the minute: {wait}");
    // The right password and a registration are attempts like any other.
    retry_after(&login(&th, "first", PASSWORD).await);
    // So is an acceptance of an invitation, whatever its token: trying tokens is
    // counted as trying passwords is.
    let late = serde_json::json!({
        "token": th.member_link().await,
        "account": {"username": "late", "email": "late@sample.com", "password": PASSWORD},
    });
    retry_after(&th.anonymous().post("/invitations/accept", &late).await);
    let guess = serde_json::json!({"token": "a-guess", "account": late["account"]});
    retry_after(&th.anonymous().post("/invitations/accept", &guess).await);
    // A registration that is refused whatever it says costs no hash and is no attempt:
    // it is answered as what it is.
    th.anonymous()
        .post("/register", &register("without-an-invitation"))
        .await
        .check_error(
            StatusCode::FORBIDDEN,
            "this server has its organization; an invitation lets you in",
        );
    // What needs no password is not an attempt.
    th.anonymous().get("/client-config").await.check_ok();

    th.teardown().await;
}

/// The queue of password work is bounded: what does not fit is told to come back, at
/// once, without a hash being spent on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn logins_beyond_the_queue_of_password_work_are_told_to_come_back() {
    let th = Setup::native()
        .configure(|config| {
            config.password_hash_concurrency = 1;
            config.password_hash_queue = 1;
            config.password_hash_queue_seconds = 2;
        })
        .start()
        .await
        .init_basic()
        .await;

    // Many at once: one is hashed, one waits, the rest are refused or wait their turn
    // in vain. None fails otherwise, and the server is as it was afterwards.
    let attempts = (0..24).map(|index| {
        let client = th.anonymous();
        async move {
            client
                .post(
                    "/login",
                    &credentials(&format!("nobody-{index}"), "a wrong password"),
                )
                .await
        }
    });
    let answers = futures_util::future::join_all(attempts).await;

    let refused: Vec<&ApiResponse> = answers
        .iter()
        .filter(|answer| answer.status == StatusCode::TOO_MANY_REQUESTS)
        .collect();
    assert!(
        !refused.is_empty(),
        "24 logins at once do not all fit a queue of one"
    );
    for answer in &refused {
        assert_eq!(retry_after(answer), 2, "come back when the queue has moved");
    }
    for answer in &answers {
        assert!(
            matches!(
                answer.status,
                StatusCode::UNAUTHORIZED | StatusCode::TOO_MANY_REQUESTS
            ),
            "refused as a wrong login or as one too many: {}",
            answer.text()
        );
    }
    login(&th, USER1_USERNAME, PASSWORD)
        .await
        .check_no_content();

    th.teardown().await;
}
