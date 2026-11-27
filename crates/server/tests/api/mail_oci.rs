//! Mail through OCI Email Delivery's HTTPS submission, faked: recovery and invitation
//! messages leave through the queue under a request signature the fake verifies with
//! the API key's public half, and OCI's answers, by their status and their code, sort
//! the attempts.

use std::time::Duration;

use fvoci_model::Invitation;
use serde_json::json;

use crate::common::{ROOT_WORKSPACE_ID, Setup, TestServer};
use crate::fake_provider::{FakeProvider, OCI_COMPARTMENT, Provider};
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
async fn recovery_and_invitation_mail_leave_through_oci_signed() {
    let fake = FakeProvider::start().await;
    let th = Setup::native()
        .mail(fake.settings(Provider::Oci))
        .start()
        .await
        .init_basic()
        .await;
    request_reset(&th).await;
    let reset = fake.taken(1).await.remove(0);
    assert!(
        reset.signed,
        "a signature OCI takes, over the body it was sent"
    );
    assert_eq!(reset.to, "user1@sample.com");
    assert_eq!(
        reset.body["sender"]["senderAddress"]["email"],
        "sender@example.test"
    );
    assert_eq!(reset.body["sender"]["compartmentId"], OCI_COMPARTMENT);
    assert!(reset.text.contains("/reset-password#"), "{}", reset.text);
    assert!(
        reset.body["bodyHtml"]
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
        assert_eq!(
            message.body["recipients"]["to"].as_array().map(Vec::len),
            Some(1)
        );
    }
    assert!(th.app.finish_background(Duration::from_secs(5)).await);
    assert_eq!(fake.recorded().await.len(), 3, "nothing more was sent");
    th.teardown().await;
    fake.stop();
}

#[tokio::test]
async fn oci_answers_sort_the_attempts_by_status_and_code() {
    let fake = FakeProvider::start().await;
    let th = Setup::native()
        .mail(fake.settings(Provider::Oci))
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

    // Past the messages a minute or a day (a 400 by its code), then too many requests:
    // later, then taken.
    fake.answer_next_named(400, "LimitExceeded").await;
    fake.answer_next(&[429]).await;
    request_reset(&th).await;
    let statuses: Vec<u16> = fake.requests(3).await.iter().map(|r| r.status).collect();
    assert_eq!(statuses, [400, 429, 200]);

    // A request OCI will not take: this message only.
    fake.answer_next_named(400, "InvalidParameter").await;
    request_reset(&th).await;
    let body = scrape_until(&th, |body| failed(body, "permanent") == Some(1.0)).await;
    assert_eq!(failed(&body, "permanent"), Some(1.0), "{body}");
    assert_eq!(
        fake.recorded().await.len(),
        4,
        "a refusal is not tried again"
    );

    // A key OCI does not take: the whole way is closed.
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
    assert_eq!(failed(&body, "temporary"), Some(0.0), "{body}");
    assert_eq!(
        failed(&body, "permanent"),
        Some(1.0),
        "a closed way fails no message: {body}"
    );
    th.teardown().await;
    fake.stop();
}

#[tokio::test]
async fn a_server_choosing_oci_without_a_usable_key_does_not_start() {
    let fake = FakeProvider::start().await;
    for key in [
        "oci_region",
        "oci_tenancy",
        "oci_user",
        "oci_fingerprint",
        "oci_private_key_file",
        "oci_compartment",
    ] {
        let (smtp, mut mail) = fake.settings(Provider::Oci);
        match key {
            "oci_region" => mail.oci_region.clear(),
            "oci_tenancy" => mail.oci_tenancy.clear(),
            "oci_user" => mail.oci_user.clear(),
            "oci_fingerprint" => mail.oci_fingerprint.clear(),
            "oci_private_key_file" => mail.oci_private_key_file.clear(),
            _ => mail.oci_compartment.clear(),
        }
        let Err(refusal) = Setup::native().mail((smtp, mail)).try_start().await else {
            panic!("{key}: the server started");
        };
        let refusal = format!("{refusal:#}");
        assert!(refusal.contains(key), "{key}: {refusal}");
    }
    let directory = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let not_a_key = directory.join(format!("not-an-oci-key-{}.pem", fake.address.port()));
    std::fs::write(&not_a_key, "a secret that is no key").expect("written");
    for file in [directory.join("no-such-oci-key.pem"), not_a_key.clone()] {
        let (smtp, mut mail) = fake.settings(Provider::Oci);
        mail.oci_private_key_file = file.display().to_string();
        let Err(refusal) = Setup::native().mail((smtp, mail)).try_start().await else {
            panic!("{}: the server started", file.display());
        };
        let refusal = format!("{refusal:#}");
        assert!(refusal.contains("oci_private_key_file"), "{refusal}");
        assert!(!refusal.contains("a secret"), "{refusal}");
    }
    let _ = std::fs::remove_file(not_a_key);
    fake.stop();
}
