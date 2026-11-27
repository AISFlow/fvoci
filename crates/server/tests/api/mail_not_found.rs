//! A provider's `404` (OCI's `NotAuthorizedOrNotFound`, Cloudflare's account it cannot
//! route to) answers the server's configuration, the same for every message: the way is
//! closed, no message is refused for it, and a mailed guest link is not taken back.

use std::time::Duration;

use fvoci_model::Invitation;
use serde_json::json;

use crate::common::{ROOT_WORKSPACE_ID, Setup};
use crate::fake_provider::{FakeProvider, Provider};
use crate::invitation_mail_children::{children, links_path};
use crate::mail_path::assert_nothing_given_up;
use crate::metrics::{sample, scrape, scrape_until};

async fn a_not_found_closes_the_way(provider: Provider) {
    let fake = FakeProvider::start().await;
    let th = Setup::native()
        .mail(fake.settings(provider))
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    let board = th
        .create_board(ROOT_WORKSPACE_ID, fvoci_model::ContainerVisibility::Open)
        .await;
    let parent: Invitation = th
        .client
        .post(
            &links_path(),
            &json!({"role":"guest","containerId":board.id,"containerRole":"viewer"}),
        )
        .await
        .created_json();
    fake.answer_next(&[404]).await;
    th.client
        .post(
            &format!("{}/mail", links_path()),
            &json!({"token":parent.token,"recipients":["guest@example.test"]}),
        )
        .await
        .check_no_content();
    let body = scrape_until(&th, |body| {
        sample(body, "fvoci_mail_path_down", &[]) == Some(1.0)
    })
    .await;
    assert_eq!(
        sample(&body, "fvoci_mail_path_down", &[]),
        Some(1.0),
        "{provider:?}: {body}"
    );

    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email":"user1@sample.com"}),
        )
        .await
        .check_no_content();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        fake.recorded().await.len(),
        1,
        "{provider:?}: the recovery and the held invitation wait for the minute's attempt"
    );
    let body = scrape(&th).await;
    assert_eq!(
        sample(&body, "fvoci_mail_path_down", &[]),
        Some(1.0),
        "{provider:?}: still closed: {body}"
    );
    for reason in ["queue_full", "account_limit"] {
        assert_eq!(
            sample(
                &body,
                "fvoci_password_reset_mail_dropped_total",
                &[("reason", reason)]
            ),
            Some(0.0),
            "{provider:?}: the recovery was queued, and waits: {body}"
        );
    }
    assert_nothing_given_up(&body);
    assert_eq!(
        children(&th, &parent).await,
        ["guest@example.test"],
        "{provider:?}: the held message's guest link is not taken back"
    );
    th.teardown().await;
    fake.stop();
}

#[tokio::test]
async fn ocis_not_authorized_or_not_found_closes_the_way() {
    a_not_found_closes_the_way(Provider::Oci).await;
}

#[tokio::test]
async fn cloudflares_not_found_closes_the_way() {
    a_not_found_closes_the_way(Provider::Cloudflare).await;
}
