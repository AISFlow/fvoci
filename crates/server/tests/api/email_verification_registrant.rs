//! Only who registered finishes a registration: the browser it was made from, which
//! keeps its marker in a cookie, or whoever knows the password it was made with. The
//! mailbox alone signs nobody in to an account whose password somebody else chose.

use std::time::Duration;

use fvoci_store::{SIGNUP_LIFETIME_MILLIS, Signup, SignupLookup, SignupStart};
use reqwest::{Method, StatusCode};
use serde_json::json;

use crate::common::{ApiResponse, Client, Setup, TestServer};
use crate::email_verification::{
    COMPLETE, SIGNUP_COOKIE, SIGNUP_PASSWORD, complete_elsewhere, complete_with_marker, link_token,
    marker_of, registration, verifying,
};
use crate::fake_smtp::FakeSmtp;

/// The registration with the user name, if it waits.
async fn waiting(th: &TestServer, username: &str) -> Option<Signup> {
    th.app
        .store()
        .live_signup(
            SignupLookup::Username(username),
            fvoci_server::auth::now_millis(),
        )
        .await
        .expect("read")
}

/// Checks the refusal of a link that would confirm, opened where the registration was
/// not made, without its password: nothing is made, and nobody is signed in.
#[track_caller]
fn check_password_asked(answer: &ApiResponse) {
    answer.check_status(StatusCode::FORBIDDEN);
    assert_eq!(
        answer.error_reason(),
        "password_required",
        "{}",
        answer.text()
    );
    assert_eq!(answer.refused_at(), "/password", "what is missing");
    assert_eq!(answer.session_cookie(), None, "no session");
    assert_eq!(marker_of(answer), None, "no marker is given or taken");
}

/// Somebody registers with another person's address and a password of their own. The
/// person opens the link of the mail: without the password nothing comes of it, so
/// nobody starts to work in an account that somebody else can sign in to.
#[tokio::test]
async fn the_link_alone_confirms_nothing_where_the_registration_was_not_made() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = verifying(&smtp).await;
    let theirs = th
        .anonymous()
        .post("/register", &registration("victor", "en"))
        .await;
    theirs.check_status(StatusCode::ACCEPTED);
    let token = link_token(&smtp.received(1).await[0]);
    // Another registration's browser is no more the registrant's than one without any.
    let others = th
        .anonymous()
        .post("/register", &registration("wanda", "en"))
        .await;
    others.check_status(StatusCode::ACCEPTED);
    let others_marker = marker_of(&others).expect("a marker");
    smtp.received(2).await;

    check_password_asked(
        &th.anonymous()
            .post(COMPLETE, &json!({"token": token}))
            .await,
    );
    check_password_asked(&complete_with_marker(&th, &token, &others_marker).await);
    check_password_asked(&complete_with_marker(&th, &token, "").await);
    check_password_asked(&complete_with_marker(&th, &token, "no-marker-of-anything").await);

    let wrong = complete_elsewhere(&th, &token, "not the password").await;
    wrong.check_status(StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.error_reason(), "invalid_credentials", "as a login's");
    assert_eq!(wrong.session_cookie(), None);
    let too_long = complete_elsewhere(&th, &token, &"x".repeat(2_000)).await;
    too_long.check_status(StatusCode::UNAUTHORIZED);

    assert!(
        th.app
            .store()
            .get_user_by_username("victor")
            .await
            .is_err_and(|error| error.is_not_found()),
        "no account"
    );
    assert!(waiting(&th, "victor").await.is_some(), "it waits still");

    // The link was not used by any of that: who registered finishes it.
    let confirmed = complete_elsewhere(&th, &token, SIGNUP_PASSWORD).await;
    confirmed.check_status(StatusCode::OK);
    assert!(confirmed.session_cookie().is_some(), "signed in");
    assert_eq!(
        marker_of(&confirmed),
        None,
        "another browser's marker is not this registration's to clear"
    );
    th.teardown().await;
    smtp.stop().await;
}

/// The marker is a secret of 256 bits in a cookie no script reads, kept as long as the
/// registration waits; the store has its digest only, and it is no session.
#[tokio::test]
async fn the_marker_is_a_cookie_of_the_browser_that_opens_nothing() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = verifying(&smtp).await;
    let answer = th
        .anonymous()
        .post("/register", &registration("xena", "en"))
        .await;
    answer.check_status(StatusCode::ACCEPTED);
    let marker = marker_of(&answer).expect("a marker");
    assert_eq!(
        answer.set_cookies(),
        [format!(
            "{SIGNUP_COOKIE}={marker}; Max-Age={}; Path=/; HttpOnly; SameSite=Lax",
            SIGNUP_LIFETIME_MILLIS / 1_000
        )],
        "kept for the seven days the registration waits"
    );
    assert_eq!(marker.len(), 43, "256 bits in base64url: {marker}");
    let signup = waiting(&th, "xena").await.expect("the registration");
    assert_eq!(signup.browser_hash.len(), 64, "a SHA-256 digest");
    assert!(!signup.browser_hash.contains(&marker), "not the secret");

    // No session: neither as the cookie it is, nor as a session's token.
    let as_it_is = th
        .anonymous()
        .request(Method::GET, "/users/me")
        .header("Cookie", format!("{SIGNUP_COOKIE}={marker}"));
    Client::send(as_it_is)
        .await
        .check_status(StatusCode::UNAUTHORIZED);
    th.client_with_token(&marker)
        .get("/users/me")
        .await
        .check_status(StatusCode::UNAUTHORIZED);
    th.teardown().await;
    smtp.stop().await;

    // Over HTTPS it has the name browsers bind to the host, and is `Secure`.
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| {
            config.mail.email_verification = true;
            config.server_url = "https://boards.example.com".to_owned();
        })
        .start()
        .await;
    let answer = th
        .anonymous()
        .post("/register", &registration("yuri", "en"))
        .await;
    answer.check_status(StatusCode::ACCEPTED);
    let cookies = answer.set_cookies();
    let [cookie] = cookies.as_slice() else {
        panic!("one cookie: {cookies:?}");
    };
    assert!(cookie.starts_with("__Host-fvoci_signup="), "{cookie}");
    assert!(
        cookie.ends_with("; Path=/; HttpOnly; SameSite=Lax; Secure"),
        "{cookie}"
    );
    // That name is the one read back, and no other; and the one cleared.
    let pair = cookie.split_once(';').expect("attributes").0;
    let token = link_token(&smtp.received(1).await[0]);
    let complete = |cookie: String| {
        let request = th
            .anonymous()
            .request(Method::POST, COMPLETE)
            .header("Cookie", cookie)
            .json(&json!({"token": token}));
        Client::send(request)
    };
    let plain = pair.replacen("__Host-fvoci_signup", SIGNUP_COOKIE, 1);
    check_password_asked(&complete(plain).await);
    let confirmed = complete(pair.to_owned()).await;
    confirmed.check_status(StatusCode::OK);
    assert!(
        confirmed
            .set_cookies()
            .contains(&"__Host-fvoci_signup=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax; Secure"),
        "{:?}",
        confirmed.set_cookies()
    );
    th.teardown().await;
    smtp.stop().await;
}

/// A page of another site cannot make a browser register: it would plant its marker
/// there, and the link would confirm a password the page chose.
#[tokio::test]
async fn a_registration_another_site_sends_plants_no_marker() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = verifying(&smtp).await;
    let body = registration("zed", "en");
    let forged = [
        th.anonymous().bare_request(Method::POST, "/register"),
        th.anonymous()
            .request(Method::POST, "/register")
            .header("Origin", "https://evil.example"),
    ];
    for request in forged {
        let answer = Client::send(request.json(&body)).await;
        answer.check_status(StatusCode::FORBIDDEN);
        assert_eq!(answer.set_cookies(), Vec::<&str>::new(), "no marker");
    }
    assert!(waiting(&th, "zed").await.is_none(), "nothing waits");
    th.teardown().await;
    smtp.stop().await;
}

/// Wrong passwords at a link are failed logins to the registration's user name: after
/// as many as lock an account, the name is locked for the link and for a login alike.
/// The link is still not used.
#[tokio::test]
async fn wrong_passwords_at_a_link_are_counted_as_failed_logins_to_its_name() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| {
            config.mail.email_verification = true;
            config.login_account_max_failures = 3;
        })
        .start()
        .await;
    let registered = th
        .anonymous()
        .post("/register", &registration("lock", "en"))
        .await;
    registered.check_status(StatusCode::ACCEPTED);
    let marker = marker_of(&registered).expect("a marker");
    let token = link_token(&smtp.received(1).await[0]);

    for _ in 0..3 {
        complete_elsewhere(&th, &token, "a guess")
            .await
            .check_status(StatusCode::UNAUTHORIZED);
    }
    let locked = complete_elsewhere(&th, &token, SIGNUP_PASSWORD).await;
    locked.check_status(StatusCode::TOO_MANY_REQUESTS);
    assert!(
        locked
            .header("retry-after")
            .parse::<u64>()
            .is_ok_and(|wait| wait >= 1),
        "says how long to wait"
    );
    th.anonymous()
        .login("lock", SIGNUP_PASSWORD)
        .await
        .check_status(StatusCode::TOO_MANY_REQUESTS);
    // Asking for the password is no attempt at it, and a link that confirms nothing is
    // told so whatever the name's lock.
    th.anonymous()
        .post(COMPLETE, &json!({"token": token}))
        .await
        .check_status(StatusCode::FORBIDDEN);
    complete_elsewhere(&th, "no-such-token", "a guess")
        .await
        .check_status(StatusCode::BAD_REQUEST);

    // The registrant's own browser needs no password, and the link still confirms.
    complete_with_marker(&th, &token, &marker)
        .await
        .check_status(StatusCode::OK);
    th.teardown().await;
    smtp.stop().await;
}

/// A registration made before browsers were told apart has no marker: its password
/// confirms it, and no cookie does.
#[tokio::test]
async fn a_registration_without_a_marker_is_confirmed_by_its_password_only() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = verifying(&smtp).await;
    let now = fvoci_server::auth::now_millis();
    let old = Signup {
        id: fvoci_model::new_id(),
        username: "olden".to_owned(),
        email: "olden@example.test".to_owned(),
        custom_name: String::new(),
        password_hash: fvoci_server::auth::password::hash_password(SIGNUP_PASSWORD)
            .expect("hashed"),
        language: String::new(),
        start: SignupStart::Organization("Olden".to_owned()),
        browser_hash: String::new(),
        created_at: now,
        expires_at: now + SIGNUP_LIFETIME_MILLIS,
    };
    th.app
        .store()
        .create_signup(&old, None, now)
        .await
        .expect("stored");
    th.anonymous()
        .post("/email-verification/request", &json!({"username": "olden"}))
        .await
        .check_no_content();
    assert!(
        th.app.finish_background(Duration::from_secs(10)).await,
        "the mail is done with"
    );
    let token = link_token(&smtp.received(1).await[0]);
    for marker in ["", "anything"] {
        check_password_asked(&complete_with_marker(&th, &token, marker).await);
    }
    complete_elsewhere(&th, &token, SIGNUP_PASSWORD)
        .await
        .check_status(StatusCode::OK);
    th.teardown().await;
    smtp.stop().await;
}

/// The check of the password takes the address's turn as a login does: a link opened
/// from an address the server tells apart is not refused for its own request.
#[tokio::test]
async fn a_link_opened_from_an_address_takes_one_turn_of_it() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| {
            config.mail.email_verification = true;
            config.trusted_proxies = vec!["127.0.0.1".to_owned()];
            config.login_pair_free_failures = 1;
            config.login_pair_base_delay_seconds = 30;
            config.login_address_attempts_per_minute = 4;
        })
        .start()
        .await;
    th.anonymous()
        .post("/register", &registration("addie", "en"))
        .await
        .check_status(StatusCode::ACCEPTED);
    let token = link_token(&smtp.received(1).await[0]);
    let from = |address: &'static str, body: serde_json::Value| {
        let request = th
            .anonymous()
            .request(Method::POST, COMPLETE)
            .header("X-Forwarded-For", address)
            .json(&body);
        Client::send(request)
    };
    let guesser = "203.0.113.7";
    check_password_asked(&from(guesser, json!({"token": token})).await);
    // One failure is free; after the next the address waits at the name, as after
    // failed logins: also with the right password.
    for _ in 0..2 {
        from(guesser, json!({"token": token, "password": "a guess"}))
            .await
            .check_status(StatusCode::UNAUTHORIZED);
    }
    let waits = from(
        guesser,
        json!({"token": token, "password": SIGNUP_PASSWORD}),
    )
    .await;
    waits.check_status(StatusCode::TOO_MANY_REQUESTS);
    assert!(
        waits
            .header("retry-after")
            .parse::<u64>()
            .is_ok_and(|wait| wait > 1),
        "the wait at the name, not a busy address: {}",
        waits.text()
    );
    // Each of the four took one turn of the address's four a minute, and no more: the
    // fifth is one too many.
    from(guesser, json!({"token": token}))
        .await
        .check_status(StatusCode::TOO_MANY_REQUESTS);
    // Another address is not held by that, and the link was not used.
    from(
        "203.0.113.8",
        json!({"token": token, "password": SIGNUP_PASSWORD}),
    )
    .await
    .check_status(StatusCode::OK);
    th.teardown().await;
    smtp.stop().await;
}
