//! Mail through Cloudflare Email Service's REST API, faked: recovery and invitation
//! messages leave through the queue under the account's token, and its answers sort
//! the attempts.

use std::time::Duration;

use fvoci_model::Invitation;
use serde_json::json;

use crate::common::{ROOT_WORKSPACE_ID, Setup, TestServer};
use crate::fake_provider::{CLOUDFLARE_TOKEN, FakeProvider, Provider};
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
async fn recovery_and_invitation_mail_leave_through_cloudflare() {
    let fake = FakeProvider::start().await;
    let th = Setup::native()
        .mail(fake.settings(Provider::Cloudflare))
        .start()
        .await
        .init_basic()
        .await;
    request_reset(&th).await;
    let reset = fake.taken(1).await.remove(0);
    assert!(reset.signed, "under the account's token");
    assert_eq!(reset.to, "user1@sample.com");
    assert_eq!(reset.body["from"], "sender@example.test");
    assert!(reset.text.contains("/reset-password#"), "{}", reset.text);
    assert!(
        reset.body["html"]
            .as_str()
            .is_some_and(|html| html.contains("lang=\"en\""))
    );

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
        assert!(
            !message
                .body
                .to_string()
                .contains(if message.to == "one@example.test" {
                    "two@example.test"
                } else {
                    "one@example.test"
                }),
            "one recipient a message"
        );
    }
    assert!(th.app.finish_background(Duration::from_secs(5)).await);
    assert_eq!(fake.recorded().await.len(), 3, "nothing more was sent");
    th.teardown().await;
    fake.stop();
}

#[tokio::test]
async fn cloudflares_answers_sort_the_attempts() {
    let fake = FakeProvider::start().await;
    let th = Setup::native()
        .mail(fake.settings(Provider::Cloudflare))
        // Four messages to one account.
        .configure(|config| config.smtp.mail_reset_per_account_hour = 4)
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

    // Throttled (429, code 10004) and an error of the provider: later, then taken.
    fake.answer_next(&[429, 500]).await;
    request_reset(&th).await;
    let statuses: Vec<u16> = fake.requests(3).await.iter().map(|r| r.status).collect();
    assert_eq!(statuses, [429, 500, 200]);

    // A request it will not take: this message only, once.
    fake.answer_next(&[400]).await;
    request_reset(&th).await;
    let body = scrape_until(&th, |body| failed(body, "permanent") == Some(1.0)).await;
    assert_eq!(failed(&body, "permanent"), Some(1.0), "{body}");
    assert_eq!(fake.recorded().await.len(), 4, "a 400 is not tried again");

    // Throttled for longer than another attempt may wait: given up at once, which only
    // the answer's Retry-After can have said.
    fake.answer_next_after(429, 3600).await;
    request_reset(&th).await;
    let body = scrape_until(&th, |body| failed(body, "temporary") == Some(1.0)).await;
    assert_eq!(failed(&body, "temporary"), Some(1.0), "{body}");
    assert_eq!(
        fake.recorded().await.len(),
        5,
        "not asked again before it said"
    );

    // A token it refuses: the whole way is closed, and no message is failed for it.
    fake.answer_next(&[401]).await;
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
    assert_eq!(failed(&body, "temporary"), Some(1.0), "{body}");
    assert_eq!(fake.recorded().await.len(), 6);
    th.teardown().await;
    fake.stop();
}

#[tokio::test]
async fn a_server_choosing_cloudflare_without_a_key_does_not_start() {
    let fake = FakeProvider::start().await;
    for key in ["cloudflare_account_id", "cloudflare_api_token"] {
        let (smtp, mut mail) = fake.settings(Provider::Cloudflare);
        match key {
            "cloudflare_account_id" => mail.cloudflare_account_id.clear(),
            _ => mail.cloudflare_api_token.clear(),
        }
        let Err(refusal) = Setup::native().mail((smtp, mail)).try_start().await else {
            panic!("{key}: the server started");
        };
        let refusal = format!("{refusal:#}");
        assert!(refusal.contains(key), "{key}: {refusal}");
        assert!(!refusal.contains(CLOUDFLARE_TOKEN), "{refusal}");
    }
    fake.stop();
}
