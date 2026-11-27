//! What invitation mail may hold of the sending: its own queue, all of it for an
//! organization alone on its server and a fair share of it among several, three of the
//! four sendings, of which one of several organizations uses two, recipients per
//! request, and messages per workspace and per organization and hour; and recovery
//! answering as always when its queue is full.

use std::time::Duration;

use fvoci_model::Invitation;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{ROOT_WORKSPACE_ID, Setup, TestServer};
use crate::fake_smtp::FakeSmtp;
use crate::metrics::{sample, scrape_until};

/// The token of a new member link of the root workspace, made by user1.
async fn member_link(th: &TestServer) -> String {
    let link: Invitation = th
        .client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/invitations"),
            &json!({"role":"member"}),
        )
        .await
        .created_json();
    link.token.expect("a new link has its token")
}

/// `count` addresses `<prefix><n><suffix>`.
fn addresses(prefix: &str, count: usize, suffix: &str) -> Vec<String> {
    (0..count).map(|n| format!("{prefix}{n}{suffix}")).collect()
}

fn mail_path() -> String {
    format!("/w/{ROOT_WORKSPACE_ID}/invitations/mail")
}

#[tokio::test]
async fn an_organization_alone_takes_the_whole_invitation_queue_and_leaves_recovery_its_own() {
    let smtp = FakeSmtp::start_on(0).await;
    smtp.hold("@held.test");
    let th = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| {
            // On PostgreSQL the server takes in others, and has one in use: it is the
            // organizations in use that count.
            if fvoci_store::testing::db_type_of_the_tests() == fvoci_store::DbType::Postgres {
                "open".clone_into(&mut config.org_creation);
            }
            config.smtp.mail_invitation_queue = 40;
        })
        .start()
        .await
        .init_basic()
        .await;
    let token = member_link(&th).await;
    // Four requests, each delivering one message at a time, fill the queue: all of it,
    // not half, and more than the sendings of invitations.
    for batch in 0..4 {
        th.client
            .post(
                &mail_path(),
                &json!({"token":token,"recipients":addresses(&format!("b{batch}-"), 10, "@held.test")}),
            )
            .await
            .check_no_content();
    }
    smtp.attempted(3).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        smtp.attempts(),
        3,
        "an organization alone uses the three sendings of invitations"
    );
    let refused = th
        .client
        .post(
            &mail_path(),
            &json!({"token":token,"recipients":["more@held.test"]}),
        )
        .await;
    refused.check_status(StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = serde_json::from_slice(&refused.body).expect("problem");
    assert_eq!(
        body["detail"], QUEUE_FULL,
        "the queue is full, not a share of it: {body}"
    );

    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email":"user1@sample.com"}),
        )
        .await
        .check_no_content();
    let messages = smtp.received(1).await;
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].recipient, "user1@sample.com");

    smtp.release();
    smtp.received(41).await;
    assert!(th.app.finish_background(Duration::from_secs(20)).await);
    assert_eq!(smtp.messages().await.len(), 41);
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn an_invitation_mail_request_names_at_most_its_recipients_limit() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    let token = member_link(&th).await;
    // Repeated addresses count: the limit is on what the request names.
    let mut recipients = addresses("r", 20, "@example.test");
    recipients.push("r0@example.test".to_owned());
    let refused = th
        .client
        .post(
            &mail_path(),
            &json!({"token":token,"recipients":recipients}),
        )
        .await;
    refused.check_status(StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&refused.body).expect("problem");
    assert_eq!(body["type"], "/problems/too-many-recipients", "{body}");
    assert_eq!(body["reason"], "too_many_recipients", "{body}");
    assert_eq!(body["limit"], 20, "{body}");
    assert!(body.to_string().contains("recipients"), "{body}");
    assert!(th.app.finish_background(Duration::from_secs(5)).await);
    assert!(smtp.messages().await.is_empty(), "nothing of it is sent");

    recipients.pop();
    th.client
        .post(
            &mail_path(),
            &json!({"token":token,"recipients":recipients}),
        )
        .await
        .check_no_content();
    smtp.received(20).await;
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn a_workspace_waits_once_it_queued_its_hourly_invitation_mail() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| {
            config.smtp.invitation_mail_recipients = 3;
            config.smtp.invitation_mail_per_hour = 5;
        })
        .start()
        .await
        .init_basic()
        .await;
    let token = member_link(&th).await;
    th.client
        .post(
            &mail_path(),
            &json!({"token":token,"recipients":addresses("a", 3, "@example.test")}),
        )
        .await
        .check_no_content();
    let refused = th
        .client
        .post(
            &mail_path(),
            &json!({"token":token,"recipients":addresses("b", 3, "@example.test")}),
        )
        .await;
    refused.check_status(StatusCode::TOO_MANY_REQUESTS);
    let wait: u64 = refused
        .headers
        .get("retry-after")
        .expect("Retry-After")
        .to_str()
        .expect("text")
        .parse()
        .expect("seconds");
    assert!((3590..=3600).contains(&wait), "{wait}");
    let body: Value = serde_json::from_slice(&refused.body).expect("problem");
    assert_eq!(body["type"], "/problems/invitation-mail-limit", "{body}");
    assert_eq!(body["retryAfter"], wait, "{body}");
    // What is left of the hour is still taken.
    th.client
        .post(
            &mail_path(),
            &json!({"token":token,"recipients":addresses("c", 2, "@example.test")}),
        )
        .await
        .check_no_content();
    smtp.received(5).await;
    assert!(th.app.finish_background(Duration::from_secs(5)).await);
    let messages = smtp.messages().await;
    assert_eq!(messages.len(), 5);
    assert!(
        messages
            .iter()
            .all(|message| !message.recipient.starts_with('b')),
        "the refused request sent nothing"
    );
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn a_full_invitation_queue_refuses_a_whole_request_before_answering() {
    let smtp = FakeSmtp::start_on(0).await;
    smtp.hold("@held.test");
    let th = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| {
            config.smtp.mail_invitation_queue = 6;
            config.smtp.invitation_mail_recipients = 3;
        })
        .start()
        .await
        .init_basic()
        .await;
    let token = member_link(&th).await;
    // The organization is alone on its server: it fills the whole queue.
    for batch in 0..2 {
        th.client
            .post(
                &mail_path(),
                &json!({"token":token,"recipients":addresses(&format!("q{batch}-"), 3, "@held.test")}),
            )
            .await
            .check_no_content();
    }
    smtp.attempted(2).await;
    let refused = th
        .client
        .post(
            &mail_path(),
            &json!({"token":token,"recipients":["late@example.test"]}),
        )
        .await;
    refused.check_status(StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = serde_json::from_slice(&refused.body).expect("problem");
    assert_eq!(body["type"], "/problems/mail-queue-full", "{body}");

    smtp.release();
    smtp.received(6).await;
    assert!(th.app.finish_background(Duration::from_secs(10)).await);
    let messages = smtp.messages().await;
    assert_eq!(messages.len(), 6, "{messages:?}");
    assert!(
        messages
            .iter()
            .all(|message| message.recipient != "late@example.test")
    );
    // The queue empties as messages leave, and the refusal used none of the hour.
    th.client
        .post(
            &mail_path(),
            &json!({"token":token,"recipients":["late@example.test"]}),
        )
        .await
        .check_no_content();
    smtp.received(7).await;
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn recovery_answers_the_same_when_its_queue_is_full_and_counts_the_drop() {
    let smtp = FakeSmtp::start_on(0).await;
    smtp.hold("@sample.com");
    let th = Setup::native()
        .smtp(smtp.settings())
        .metrics_at("127.0.0.1:0")
        .configure(|config| config.smtp.mail_reset_queue = 1)
        .start()
        .await
        .init_basic()
        .await;
    let first = th
        .anonymous()
        .post(
            "/password-reset/request",
            &json!({"email":"user1@sample.com"}),
        )
        .await;
    smtp.attempted(1).await;
    let dropped = th
        .anonymous()
        .post(
            "/password-reset/request",
            &json!({"email":"user2@sample.com"}),
        )
        .await;
    for answer in [&first, &dropped] {
        answer.check_status(StatusCode::NO_CONTENT);
        assert_eq!(answer.body, b"");
    }
    let headers = |answer: &crate::common::ApiResponse| {
        let mut names: Vec<String> = answer
            .headers
            .keys()
            .map(|name| name.as_str().to_owned())
            .filter(|name| name != "date" && name != "x-request-id")
            .collect();
        names.sort();
        names
    };
    assert_eq!(headers(&first), headers(&dropped));
    let body = scrape_until(&th, |body| {
        sample(
            body,
            "fvoci_password_reset_mail_dropped_total",
            &[("reason", "queue_full")],
        ) == Some(1.0)
    })
    .await;
    assert!(!body.contains("user2"), "{body}");

    smtp.release();
    smtp.received(1).await;
    assert!(th.app.finish_background(Duration::from_secs(10)).await);
    let messages = smtp.messages().await;
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].recipient, "user1@sample.com");
    assert_eq!(
        th.app
            .store()
            .count_rows("password_resets")
            .await
            .expect("tokens"),
        1,
        "the dropped request made no token"
    );
    th.teardown().await;
    smtp.stop().await;
}

/// A second workspace of user1's organization, which user1 administers, and the token
/// of a new member link of it.
async fn second_workspace_link(th: &TestServer) -> (String, String) {
    let own: Vec<fvoci_model::UserWorkspace> =
        th.client.get("/users/me/workspaces").await.ok_json();
    let org = own.first().expect("user1's organization").org_id.clone();
    let made: Value = th
        .client
        .post(
            &format!("/org/{org}/workspaces"),
            &json!({"name": "Mail workspace"}),
        )
        .await
        .created_json();
    let workspace = made["id"].as_str().expect("an identifier").to_owned();
    th.client
        .post_empty(&format!("/org/{org}/workspaces/{workspace}/join"))
        .await
        .check_ok();
    let link: Invitation = th
        .client
        .post(
            &format!("/w/{workspace}/invitations"),
            &json!({"role":"member"}),
        )
        .await
        .created_json();
    (workspace, link.token.expect("a new link has its token"))
}

#[tokio::test]
async fn an_organization_waits_once_its_workspaces_queued_its_hourly_invitation_mail() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| {
            config.smtp.invitation_mail_recipients = 3;
            // A workspace's hour is never above its organization's.
            config.smtp.invitation_mail_per_hour = 5;
            config.smtp.invitation_mail_per_org_hour = 5;
        })
        .start()
        .await
        .init_basic()
        .await;
    assert_eq!(th.app.limits().invitation_mail_per_hour, 5);
    let token = member_link(&th).await;
    let (workspace, other_token) = second_workspace_link(&th).await;
    th.client
        .post(
            &mail_path(),
            &json!({"token":token,"recipients":addresses("a", 3, "@example.test")}),
        )
        .await
        .check_no_content();
    let other_path = format!("/w/{workspace}/invitations/mail");
    let refused = th
        .client
        .post(
            &other_path,
            &json!({"token":other_token,"recipients":addresses("b", 3, "@example.test")}),
        )
        .await;
    refused.check_status(StatusCode::TOO_MANY_REQUESTS);
    let body: Value = serde_json::from_slice(&refused.body).expect("problem");
    assert_eq!(body["type"], "/problems/invitation-mail-limit", "{body}");
    assert!(
        body.to_string().contains("organization"),
        "the organization's limit, not the workspace's: {body}"
    );
    let wait = body["retryAfter"].as_u64().expect("seconds");
    assert!((3590..=3600).contains(&wait), "{wait}");
    th.client
        .post(
            &other_path,
            &json!({"token":other_token,"recipients":addresses("c", 2, "@example.test")}),
        )
        .await
        .check_no_content();
    smtp.received(5).await;
    assert!(th.app.finish_background(Duration::from_secs(10)).await);
    assert_eq!(smtp.messages().await.len(), 5);
    th.teardown().await;
    smtp.stop().await;
}

/// What the queue's refusal says, and what a share's says.
const QUEUE_FULL: &str = "too much mail is waiting; try again later";
const SHARE_FULL: &str = "the organization has too much mail waiting; try again later";

/// Who mails a member link of their workspace.
struct Founder {
    client: crate::common::Client,
    /// The path the link is mailed through.
    path: String,
    token: String,
}

/// A founder of an organization of their own on the server, signed in, with a member
/// link of their workspace.
async fn founder(th: &TestServer, name: &str) -> Founder {
    let mut client = th.anonymous();
    client
        .post(
            "/register",
            &json!({"organizationName":format!("{name} mail"),"username":name,
                "email":format!("{name}@example.test"),"password":"Founder-password"}),
        )
        .await
        .check_status(StatusCode::CREATED);
    client
        .login(name, "Founder-password")
        .await
        .check_no_content();
    let workspaces: Vec<fvoci_model::UserWorkspace> =
        client.get("/users/me/workspaces").await.ok_json();
    let own = &workspaces[0].id;
    let link: Invitation = client
        .post(&format!("/w/{own}/invitations"), &json!({"role":"member"}))
        .await
        .created_json();
    Founder {
        client,
        path: format!("/w/{own}/invitations/mail"),
        token: link.token.expect("a new link has its token"),
    }
}

/// Mails the founder's link to five held addresses of `prefix` at a time, until the
/// organization's share refuses (503); how many requests the share took.
async fn fill(founder: &Founder, prefix: &str) -> usize {
    for taken in 0.. {
        let answer = founder
            .client
            .post(
                &founder.path,
                &json!({"token":founder.token,"recipients":addresses(&format!("{prefix}{taken}-"), 5, "@held.test")}),
            )
            .await;
        if answer.status == StatusCode::SERVICE_UNAVAILABLE {
            let body: Value = serde_json::from_slice(&answer.body).expect("problem");
            assert_eq!(
                body["detail"], SHARE_FULL,
                "its share, not the queue: {body}"
            );
            return taken;
        }
        answer.check_no_content();
    }
    unreachable!("a share ends")
}

#[tokio::test]
async fn three_busy_organizations_leave_a_fourth_room_for_its_invitations() {
    // Several organizations are a PostgreSQL deployment; SQLite refuses that mode.
    if fvoci_store::testing::db_type_of_the_tests() != fvoci_store::DbType::Postgres {
        return;
    }
    let smtp = FakeSmtp::start_on(0).await;
    smtp.hold("@held.test");
    let th = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| {
            "open".clone_into(&mut config.org_creation);
            config.smtp.mail_invitation_queue = 40;
            config.smtp.invitation_mail_recipients = 5;
        })
        .start()
        .await
        .init_basic()
        .await;
    let first = Founder {
        client: th.client.clone(),
        path: mail_path(),
        token: member_link(&th).await,
    };
    let second = founder(&th, "mail-second").await;
    let third = founder(&th, "mail-third").await;
    let fourth = founder(&th, "mail-fourth").await;

    // Each busy organization takes half of what the others leave: 20, 10, then one
    // request of 5.
    let taken = [
        fill(&first, "a").await,
        fill(&second, "b").await,
        fill(&third, "c").await,
    ];
    assert_eq!(taken, [4, 2, 1], "requests of five of a queue of 40");
    fourth
        .client
        .post(
            &fourth.path,
            &json!({"token":fourth.token,"recipients":addresses("d", 5, "@example.test")}),
        )
        .await
        .check_no_content();

    smtp.release();
    let delivered = smtp.received(40).await;
    assert!(th.app.finish_background(Duration::from_secs(30)).await);
    assert_eq!(
        delivered
            .iter()
            .filter(|message| message.recipient.ends_with("@example.test"))
            .count(),
        5,
        "the fourth organization's invitations went out"
    );
    assert_eq!(smtp.messages().await.len(), 40);
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn an_organizations_hour_below_a_workspaces_refuses_the_start_by_its_keys() {
    let smtp = FakeSmtp::start_on(0).await;
    let Err(refusal) = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| {
            config.smtp.invitation_mail_per_hour = 120;
            config.smtp.invitation_mail_per_org_hour = 110;
        })
        .try_start()
        .await
    else {
        panic!("the server started");
    };
    let refusal = format!("{refusal:#}");
    for said in [
        "invitation_mail_per_org_hour",
        "110",
        "invitation_mail_per_hour",
        "120",
    ] {
        assert!(refusal.contains(said), "{said}: {refusal}");
    }
    smtp.stop().await;
}
