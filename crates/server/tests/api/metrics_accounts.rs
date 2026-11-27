//! The metrics of the accounts: how logins ended, how many users registered, how many
//! sessions are live; and nothing in them names a user.

use reqwest::StatusCode;

use crate::common::{PASSWORD, Setup, USER1_USERNAME, USER2_USERNAME};
use crate::metrics::{sample, scrape};

/// How many logins ended so.
fn logins(body: &str, outcome: &str) -> Option<f64> {
    sample(body, "fvoci_logins_total", &[("outcome", outcome)])
}

#[tokio::test]
async fn logins_registrations_and_live_sessions_are_counted() {
    let server = Setup::native()
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    let user_ids = [server.client.me().await.id, server.client2.me().await.id];
    let attempt = async |username: &str, password: &str| {
        server
            .anonymous()
            .post(
                "/login",
                &serde_json::json!({"type": "normal", "username": username, "password": password}),
            )
            .await
            .status
    };
    // A wrong password and an account that is not there are one outcome.
    assert_eq!(
        attempt(USER1_USERNAME, "not the password").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        attempt("nobody-at-all", PASSWORD).await,
        StatusCode::UNAUTHORIZED
    );
    let body = scrape(&server).await;
    assert_eq!(
        logins(&body, "succeeded"),
        Some(2.0),
        "the two of init_basic: {body}"
    );
    assert_eq!(logins(&body, "invalid_credentials"), Some(2.0), "{body}");
    assert_eq!(logins(&body, "limited"), Some(0.0), "{body}");
    assert_eq!(logins(&body, "failed"), Some(0.0), "{body}");
    assert_eq!(logins(&body, "abandoned"), Some(0.0), "{body}");
    assert_eq!(logins(&body, "email_unverified"), Some(0.0), "{body}");
    assert_eq!(
        body.lines()
            .filter(|line| line.starts_with("fvoci_logins_total{"))
            .count(),
        6,
        "six outcomes, whatever was asked: {body}"
    );
    assert_eq!(
        sample(&body, "fvoci_registrations_total", &[]),
        Some(2.0),
        "{body}"
    );
    assert_eq!(
        sample(&body, "fvoci_sessions_live", &[]),
        Some(2.0),
        "{body}"
    );
    for named in [
        USER1_USERNAME,
        USER2_USERNAME,
        "nobody-at-all",
        "sample.com",
    ]
    .into_iter()
    .chain(user_ids.iter().map(String::as_str))
    {
        assert!(!body.contains(named), "{named} is in no metric: {body}");
    }

    assert_eq!(
        sample(&body, "fvoci_sessions_read_failures_total", &[]),
        Some(0.0),
        "{body}"
    );

    // A registration that stores no user is not counted.
    let response = server
        .anonymous()
        .post(
            "/register",
            &serde_json::json!({
                "username": USER1_USERNAME, "email": "another@example.com",
                "password": PASSWORD, "token": "",
            }),
        )
        .await;
    assert!(response.status.is_client_error(), "{}", response.status);
    let body = scrape(&server).await;
    assert_eq!(
        sample(&body, "fvoci_registrations_total", &[]),
        Some(2.0),
        "{body}"
    );
    server.teardown().await;
}

/// A login with a name and an email address reads the same, whether the name is an
/// account or not: the count of the database's reads tells nothing of it.
#[tokio::test]
async fn a_login_reads_alike_whether_its_name_is_an_account() {
    let server = Setup::native()
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    let reads = |body: &str| {
        sample(
            body,
            "fvoci_db_call_duration_seconds_count",
            &[
                ("access", "read"),
                ("db", server.app.store().db_type().as_str()),
            ],
        )
        .unwrap_or_default()
    };
    let attempt = async |username: &str| {
        let status = server
            .anonymous()
            .post(
                "/login",
                &serde_json::json!({
                    "type": "normal", "username": username,
                    "email": "nobody@example.com", "password": "not the password",
                }),
            )
            .await
            .status;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{username}");
    };
    // The scrapes after the first read the same each: within five seconds the
    // readiness is kept, and the live sessions within a minute.
    scrape(&server).await;
    let before = reads(&scrape(&server).await);
    attempt(USER1_USERNAME).await;
    let between = reads(&scrape(&server).await);
    attempt("nobody-at-all").await;
    let after = reads(&scrape(&server).await);
    assert_eq!(
        between - before,
        after - between,
        "an account and no account read alike"
    );
    server.teardown().await;
}
