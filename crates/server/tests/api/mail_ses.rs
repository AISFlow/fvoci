//! Mail through Amazon SES's API v2, faked: recovery and invitation messages leave
//! through the queue as raw MIME under a Signature Version 4 the fake checks, and SES's
//! answers, by their status and their error's name, sort the attempts.

use std::time::Duration;

use fvoci_model::Invitation;
use serde_json::json;

use crate::common::{ROOT_WORKSPACE_ID, Setup, TestServer};
use crate::fake_provider::{FakeProvider, Provider, SES_SECRET};
use crate::metrics::{sample, scrape_until};

async fn request_reset(th: &TestServer) {
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email":"user1@sample.com"}),
        )
        .await
        .check_no_content();
}

#[tokio::test]
async fn recovery_and_invitation_mail_leave_through_ses_signed() {
    let fake = FakeProvider::start().await;
    let th = Setup::native()
        .mail(fake.settings(Provider::Ses))
        .start()
        .await
        .init_basic()
        .await;
    request_reset(&th).await;
    let reset = fake.taken(1).await.remove(0);
    assert!(reset.signed, "a signature SES takes");
    assert_eq!(reset.to, "user1@sample.com");
    assert_eq!(reset.body["FromEmailAddress"], "sender@example.test");
    assert!(reset.text.contains("/reset-password#"), "{}", reset.text);
    assert!(
        reset.mime.contains("Content-Language: en"),
        "{}",
        reset.mime
    );
    assert!(!reset.body.to_string().contains(SES_SECRET));

    let path = format!("/w/{ROOT_WORKSPACE_ID}/invitations");
    let link: Invitation = th
        .client
        .post(&path, &json!({"role":"member"}))
        .await
        .created_json();
    let token = link.token.expect("a new link has its token");
    th.client
        .post(
            &format!("{path}/mail"),
            &json!({"token":token,"recipients":["one@example.test","two@example.test"]}),
        )
        .await
        .check_no_content();
    let messages = fake.taken(3).await;
    let mut invited: Vec<&str> = messages[1..]
        .iter()
        .map(|message| message.to.as_str())
        .collect();
    invited.sort_unstable();
    assert_eq!(invited, ["one@example.test", "two@example.test"]);
    for message in &messages[1..] {
        assert!(message.signed);
        assert!(message.text.contains(&format!("/invite/{token}")));
        assert_eq!(
            message.body["Destination"]["ToAddresses"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
    }
    assert!(th.app.finish_background(Duration::from_secs(5)).await);
    assert_eq!(fake.recorded().await.len(), 3, "nothing more was sent");
    th.teardown().await;
    fake.stop();
}

#[tokio::test]
async fn ses_answers_sort_the_attempts_by_status_and_name() {
    let fake = FakeProvider::start().await;
    let th = Setup::native()
        .mail(fake.settings(Provider::Ses))
        .configure(|config| config.smtp.mail_reset_per_account_hour = 3)
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    let failed = |body: &str, class: &str| {
        sample(
            body,
            "fvoci_mail_failed_total",
            &[("class", class), ("kind", "password_reset")],
        )
    };

    // Past the sending rate or the day's quota (a 400 by its name), then an error of
    // the service: later, then taken.
    fake.answer_next_named(400, "ThrottlingException").await;
    fake.answer_next(&[500]).await;
    request_reset(&th).await;
    let statuses: Vec<u16> = fake.requests(3).await.iter().map(|r| r.status).collect();
    assert_eq!(statuses, [400, 500, 200]);

    // A message SES will not carry: this message only, once.
    fake.answer_next_named(400, "MessageRejected").await;
    request_reset(&th).await;
    let body = scrape_until(&th, |body| failed(body, "permanent") == Some(1.0)).await;
    assert_eq!(failed(&body, "permanent"), Some(1.0), "{body}");
    assert_eq!(
        fake.recorded().await.len(),
        4,
        "a refusal is not tried again"
    );

    // A paused account (a 400 by its name): the whole way is closed.
    fake.answer_next_named(400, "SendingPausedException").await;
    request_reset(&th).await;
    let body = scrape_until(&th, |body| {
        sample(body, "fvoci_mail_path_down", &[]) == Some(1.0)
    })
    .await;
    assert_eq!(
        sample(&body, "fvoci_mail_path_down", &[]),
        Some(1.0),
        "{body}"
    );
    assert_eq!(failed(&body, "permanent"), Some(1.0), "{body}");
    assert_eq!(failed(&body, "temporary"), Some(0.0), "{body}");
    th.teardown().await;
    fake.stop();
}

#[tokio::test]
async fn a_signature_ses_refuses_closes_the_way() {
    let fake = FakeProvider::start().await;
    let th = Setup::native()
        .mail(fake.settings(Provider::Ses))
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    fake.answer_next(&[403]).await;
    request_reset(&th).await;
    let body = scrape_until(&th, |body| {
        sample(body, "fvoci_mail_path_down", &[]) == Some(1.0)
    })
    .await;
    assert_eq!(
        sample(&body, "fvoci_mail_path_down", &[]),
        Some(1.0),
        "{body}"
    );
    th.teardown().await;
    fake.stop();
}

#[tokio::test]
async fn a_server_choosing_ses_without_a_key_does_not_start() {
    let fake = FakeProvider::start().await;
    for key in ["ses_region", "ses_access_key_id", "ses_secret_access_key"] {
        let (smtp, mut mail) = fake.settings(Provider::Ses);
        match key {
            "ses_region" => mail.ses_region.clear(),
            "ses_access_key_id" => mail.ses_access_key_id.clear(),
            _ => mail.ses_secret_access_key.clear(),
        }
        let Err(refusal) = Setup::native().mail((smtp, mail)).try_start().await else {
            panic!("{key}: the server started");
        };
        let refusal = format!("{refusal:#}");
        assert!(refusal.contains(key), "{key}: {refusal}");
        assert!(!refusal.contains(SES_SECRET), "{refusal}");
    }
    fake.stop();
}
