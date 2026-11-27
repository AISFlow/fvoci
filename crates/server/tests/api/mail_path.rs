//! A way of sending closed to the server as a whole (a relay that wants credentials,
//! one that cannot be reached): no message is refused for it, invitations wait, and the
//! metric says so.

use std::time::Duration;

use fvoci_model::Invitation;
use serde_json::json;

use crate::common::{ROOT_WORKSPACE_ID, Setup, TestServer};
use crate::fake_smtp::FakeSmtp;
use crate::metrics::{sample, scrape_until};

/// Asks for a recovery message for user1, and waits until the way is closed.
async fn close_the_way_with_a_reset(th: &TestServer) -> String {
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email":"user1@sample.com"}),
        )
        .await
        .check_no_content();
    scrape_until(th, |body| {
        sample(body, "fvoci_mail_path_down", &[]) == Some(1.0)
    })
    .await
}

/// No message of either kind was given up.
pub(crate) fn assert_nothing_given_up(body: &str) {
    for kind in ["password_reset", "invitation"] {
        for class in ["permanent", "temporary"] {
            assert_eq!(
                sample(
                    body,
                    "fvoci_mail_failed_total",
                    &[("class", class), ("kind", kind)]
                ),
                Some(0.0),
                "{kind} {class}: a closed way gives no message up at once"
            );
        }
    }
}

#[tokio::test]
async fn a_relay_that_wants_credentials_closes_the_way_and_invitations_wait() {
    let smtp = FakeSmtp::start_on(0).await;
    smtp.require_credentials_next(1);
    let th = Setup::native()
        .smtp(smtp.settings())
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    let body = close_the_way_with_a_reset(&th).await;
    assert_eq!(
        sample(&body, "fvoci_mail_path_down", &[]),
        Some(1.0),
        "{body}"
    );
    assert_nothing_given_up(&body);
    assert_eq!(smtp.attempts(), 1);

    let link: Invitation = th
        .client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/invitations"),
            &json!({"role":"member"}),
        )
        .await
        .created_json();
    th.client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/invitations/mail"),
            &json!({"token":link.token,"recipients":["new@example.test"]}),
        )
        .await
        .check_no_content();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        smtp.attempts(),
        1,
        "nothing more is tried until the minute's attempt"
    );
    assert!(smtp.messages().await.is_empty());
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn a_relay_that_cannot_be_reached_as_asked_closes_the_way() {
    use fvoci_server::config::smtp::SmtpTls;
    // The sink speaks plain SMTP and offers no STARTTLS: neither a TLS handshake nor
    // the upgrade can be made.
    for tls in [SmtpTls::Tls, SmtpTls::Starttls] {
        let smtp = FakeSmtp::start_on(0).await;
        let mut settings = smtp.settings();
        settings.smtp_tls = tls;
        let th = Setup::native()
            .smtp(settings)
            .metrics_at("127.0.0.1:0")
            .start()
            .await
            .init_basic()
            .await;
        let body = close_the_way_with_a_reset(&th).await;
        assert_eq!(
            sample(&body, "fvoci_mail_path_down", &[]),
            Some(1.0),
            "{tls:?}: {body}"
        );
        assert_nothing_given_up(&body);
        assert!(smtp.messages().await.is_empty());
        th.teardown().await;
        smtp.stop().await;
    }
}
