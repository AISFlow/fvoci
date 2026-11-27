//! SMTP recovery: uniform requests, native accounts only, bounded retries and throttling.

use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde_json::json;

use crate::common::Setup;
use crate::fake_smtp::FakeSmtp;
use crate::metrics::{sample, scrape_until};

#[tokio::test]
async fn mail_reset_answers_uniformly_without_waiting_for_delivery() {
    let smtp = FakeSmtp::start_on(0).await;
    smtp.delay(Duration::from_secs(2)).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    let passwordless = th
        .app
        .store()
        .get_user_by_username("user2")
        .await
        .expect("user");
    th.app
        .store()
        .replace_password_hash(&passwordless.id, &passwordless.password_hash, "")
        .await
        .expect("passwordless");
    let mut times = Vec::new();
    for email in [
        "user1@sample.com",
        "absent@example.test",
        "user2@sample.com",
    ] {
        let started = Instant::now();
        let answer = tokio::time::timeout(
            Duration::from_secs(1),
            th.anonymous().post(
                "/password-reset/request",
                &json!({"email":email,"language":"ko"}),
            ),
        )
        .await
        .expect("request does not wait for SMTP");
        times.push(started.elapsed());
        answer.check_no_content();
    }
    assert!(
        times
            .iter()
            .max()
            .expect("timings")
            .saturating_sub(*times.iter().min().expect("timings"))
            < Duration::from_millis(250),
        "response timings {times:?}"
    );
    assert!(
        smtp.messages().await.is_empty(),
        "responses precede slow delivery"
    );
    let messages = smtp.received(1).await;
    assert!(th.app.finish_background(Duration::from_secs(5)).await);
    assert_eq!(
        smtp.messages().await.len(),
        1,
        "absent and passwordless accounts receive nothing"
    );
    let message = &messages[0];
    assert_eq!(message.recipient, "user1@sample.com");
    let decoded = &message.decoded;
    assert!(message.message.contains("text/plain") && message.message.contains("text/html"));
    assert!(
        decoded.contains("lang=\"ko\""),
        "request language used when recipient is auto"
    );
    assert!(message.message.contains("Content-Language: ko"));
    assert!(!decoded.contains(&passwordless.id));
    let link = decoded
        .lines()
        .find(|line| line.starts_with("http") && line.contains("/reset-password#"))
        .expect("plain reset link");
    let token = link.split_once('#').expect("fragment").1.trim();
    th.anonymous()
        .post(
            "/password-reset/complete",
            &json!({"token":token,"password":"New-password-1"}),
        )
        .await
        .check_no_content();
    th.client
        .get("/users/me")
        .await
        .check_status(StatusCode::UNAUTHORIZED);
    th.anonymous()
        .login("user1", "New-password-1")
        .await
        .check_no_content();
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn mail_reset_retries_twice_and_stops() {
    let smtp = FakeSmtp::start_on(0).await;
    smtp.fail_next(usize::MAX);
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email":"user1@sample.com"}),
        )
        .await
        .check_no_content();
    assert!(th.app.finish_background(Duration::from_secs(10)).await);
    assert_eq!(smtp.attempts(), 3);
    assert!(smtp.messages().await.is_empty());
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn mail_reset_refused_for_good_is_not_tried_again_and_is_counted() {
    let smtp = FakeSmtp::start_on(0).await;
    smtp.reject_next(1);
    let th = Setup::native()
        .smtp(smtp.settings())
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email":"user1@sample.com"}),
        )
        .await
        .check_no_content();
    assert!(th.app.finish_background(Duration::from_secs(10)).await);
    assert_eq!(smtp.attempts(), 1, "a 5xx reply is final");
    assert!(smtp.messages().await.is_empty());
    let body = scrape_until(&th, |body| {
        sample(
            body,
            "fvoci_mail_failed_total",
            &[("class", "permanent"), ("kind", "password_reset")],
        ) == Some(1.0)
    })
    .await;
    assert_eq!(
        sample(
            &body,
            "fvoci_mail_failed_total",
            &[("class", "temporary"), ("kind", "password_reset")],
        ),
        Some(0.0)
    );
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn mail_reset_retries_a_transient_failure() {
    let smtp = FakeSmtp::start_on(0).await;
    smtp.fail_next(1);
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email":"user1@sample.com"}),
        )
        .await
        .check_no_content();
    smtp.received(1).await;
    assert_eq!(smtp.attempts(), 2);
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn mail_reset_limits_addresses_across_emails_and_never_an_email() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| {
            config.login_account_max_failures = 2;
            config.login_address_attempts_per_minute = 2;
        })
        .start()
        .await;
    let ip = |last| Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, last)));
    for last in 1..=5 {
        th.app.spawn_background(
            th.app
                .prepare_password_reset(" ABSENT@example.test ", "en", ip(last))
                .expect("an email is never refused for what others asked in its name"),
        );
    }
    th.app.spawn_background(
        th.app
            .prepare_password_reset("one@example.test", "en", ip(9))
            .expect("first address"),
    );
    th.app.spawn_background(
        th.app
            .prepare_password_reset("two@example.test", "en", ip(9))
            .expect("second address"),
    );
    assert!(
        th.app
            .prepare_password_reset("three@example.test", "en", ip(9))
            .is_err(),
        "address counted across emails"
    );
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn mail_reset_sends_an_account_its_hourly_messages_and_locks_nothing() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .metrics_at("127.0.0.1:0")
        .configure(|config| config.login_account_max_failures = 2)
        .start()
        .await
        .init_basic()
        .await;
    assert_eq!(th.app.config().smtp.mail_reset_per_account_hour, 3);
    let ip = |last| Some(IpAddr::V4(Ipv4Addr::new(198, 51, 100, last)));
    for last in 1..=5 {
        let email = if last % 2 == 0 {
            "USER1@sample.com"
        } else {
            "user1@sample.com"
        };
        th.app.spawn_background(
            th.app
                .prepare_password_reset(email, "en", ip(last))
                .expect("answered as any other"),
        );
    }
    smtp.received(3).await;
    assert!(th.app.finish_background(Duration::from_secs(10)).await);
    assert_eq!(smtp.messages().await.len(), 3, "three an hour");
    assert_eq!(
        th.app
            .store()
            .count_rows("password_resets")
            .await
            .expect("tokens"),
        3,
        "a dropped request made no token"
    );
    let body = scrape_until(&th, |body| {
        sample(
            body,
            "fvoci_password_reset_mail_dropped_total",
            &[("reason", "account_limit")],
        ) == Some(2.0)
    })
    .await;
    assert_eq!(
        sample(
            &body,
            "fvoci_password_reset_mail_dropped_total",
            &[("reason", "queue_full")],
        ),
        Some(0.0)
    );
    // Neither the owner's sign-in nor another account's recovery is held back.
    th.anonymous()
        .login("user1", crate::common::PASSWORD)
        .await
        .check_no_content();
    th.app.spawn_background(
        th.app
            .prepare_password_reset("user2@sample.com", "en", ip(7))
            .expect("another account"),
    );
    smtp.received(4).await;
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn mail_reset_is_unavailable_without_smtp_and_smtp_does_not_gate_readiness() {
    let th = Setup::native().start().await;
    assert_eq!(
        th.anonymous().get("/login-options").await.value()["mailEnabled"],
        false
    );
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email":"absent@example.test"}),
        )
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.teardown().await;
    let smtp = FakeSmtp::start_on(0).await;
    let settings = smtp.settings();
    smtp.stop().await;
    let th = Setup::native().smtp(settings).start().await;
    assert_eq!(
        th.anonymous().get("/login-options").await.value()["mailEnabled"],
        true
    );
    tokio::time::timeout(Duration::from_secs(2), th.anonymous().get("/ready"))
        .await
        .expect("readiness ignores unreachable SMTP")
        .check_status(StatusCode::OK);
    th.teardown().await;
}

#[tokio::test]
async fn mail_reset_waits_for_delivery_capacity_before_minting_a_token() {
    let smtp = FakeSmtp::start_on(0).await;
    smtp.delay(Duration::from_secs(2)).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| config.smtp.mail_reset_per_account_hour = 5)
        .start()
        .await
        .init_basic()
        .await;
    for _ in 0..4 {
        th.app.spawn_background(
            th.app
                .prepare_password_reset("user1@sample.com", "en", None)
                .expect("request"),
        );
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while smtp.attempts() < 4 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("all SMTP slots are busy");
    th.app.spawn_background(
        th.app
            .prepare_password_reset("user1@sample.com", "en", None)
            .expect("queued request"),
    );
    assert!(!th.app.finish_background(Duration::from_millis(100)).await);
    assert_eq!(
        th.app
            .store()
            .count_rows("password_resets")
            .await
            .expect("tokens"),
        4,
        "a queued request's one-hour lifetime has not started"
    );
    smtp.received(5).await;
    assert_eq!(
        th.app
            .store()
            .count_rows("password_resets")
            .await
            .expect("tokens"),
        5
    );
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn mail_reset_waits_until_the_http_response_is_released() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt as _;

    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    let router = fvoci_server::api::router(&th.app).with_state(th.app.clone());
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/password-reset/request")
                .header("X-Requested-With", "XMLHttpRequest")
                .header("Content-Type", "application/json")
                .body(Body::from(json!({"email":"user1@sample.com"}).to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(th.app.finish_background(Duration::from_secs(5)).await);
    assert!(
        smtp.messages().await.is_empty(),
        "holding the response must hold delivery"
    );
    assert_eq!(
        th.app
            .store()
            .count_rows("password_resets")
            .await
            .expect("tokens"),
        0
    );
    drop(response);
    smtp.received(1).await;
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn mail_reset_is_written_in_the_recipients_language_else_the_requesters_else_english() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    th.client
        .patch("/users/me/settings", &json!({"language":"ja"}))
        .await
        .check_status(StatusCode::OK);
    for (count, (requested, want)) in [("ko-KR", "ko"), ("ja", "en")].into_iter().enumerate() {
        th.anonymous()
            .post(
                "/password-reset/request",
                &json!({"email":"user1@sample.com","language":requested}),
            )
            .await
            .check_no_content();
        let messages = smtp.received(count + 1).await;
        let last = &messages[count];
        assert!(
            last.decoded.contains(&format!("lang=\"{want}\"")),
            "{requested}: {}",
            last.decoded
        );
    }
    th.client2
        .patch("/users/me/settings", &json!({"language":"ko"}))
        .await
        .check_status(StatusCode::OK);
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email":"user2@sample.com","language":"en"}),
        )
        .await
        .check_no_content();
    let messages = smtp.received(3).await;
    assert!(
        messages[2].decoded.contains("lang=\"ko\""),
        "the recipient's own language first"
    );
    th.teardown().await;
    smtp.stop().await;
}
