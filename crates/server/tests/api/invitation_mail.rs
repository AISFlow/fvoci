//! Existing invitation links sent as individual messages under their original permissions.

use crate::common::{ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID};
use crate::fake_smtp::FakeSmtp;
use fvoci_model::Invitation;
use reqwest::StatusCode;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn invitation_mail_delivers_the_same_link_privately_in_recipient_or_sender_language() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    th.client2
        .patch("/users/me/settings", &json!({"language":"ko"}))
        .await
        .check_status(StatusCode::OK);
    let path = format!("/w/{ROOT_WORKSPACE_ID}/invitations");
    let link: Invitation = th
        .client
        .post(&path, &json!({"role":"member"}))
        .await
        .created_json();
    let before: Vec<Invitation> = th.client.get(&path).await.ok_json();
    let token = link.token.as_deref().expect("token");
    th.client.post(&format!("{path}/mail"), &json!({"token":token,"recipients":["user2@sample.com","outside@example.test"," outside@example.test "],"language":"en"})).await.check_no_content();
    let messages = smtp.received(2).await;
    assert!(th.app.finish_background(Duration::from_secs(5)).await);
    assert_eq!(
        smtp.messages().await.len(),
        2,
        "duplicates receive one message"
    );
    for message in messages {
        assert!(message.decoded.contains(&format!("/invite/{token}")));
        assert!(!message.decoded.contains(&link.id) && !message.decoded.contains(&link.created_by));
        assert!(!message.decoded.contains(ROOT_WORKSPACE_ID));
        if message.recipient == "user2@sample.com" {
            assert!(message.decoded.contains("lang=\"ko\""));
            assert!(!message.message.contains("outside@example.test"));
        } else {
            assert!(message.decoded.contains("lang=\"en\""));
            assert!(!message.message.contains("user2@sample.com"));
        }
    }
    let links: Vec<Invitation> = th.client.get(&path).await.ok_json();
    assert_eq!(
        links, before,
        "mail leaves every invitation and its use count unchanged"
    );
    th.anonymous()
        .post("/invitations/preview", &json!({"token":token}))
        .await
        .check_status(StatusCode::OK);
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn invitation_mail_rejects_unauthorized_foreign_revoked_and_injected_requests() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    let path = format!("/w/{ROOT_WORKSPACE_ID}/invitations");
    let link: Invitation = th
        .client
        .post(&path, &json!({"role":"member"}))
        .await
        .created_json();
    let request = json!({"token":link.token,"recipients":["outside@example.test"]});
    th.anonymous()
        .post(&format!("{path}/mail"), &request)
        .await
        .check_status(StatusCode::UNAUTHORIZED);
    th.client2
        .post(&format!("{path}/mail"), &request)
        .await
        .check_status(StatusCode::FORBIDDEN);
    th.client
        .post(
            &format!("/w/{TEST_WORKSPACE_ID}/invitations/mail"),
            &request,
        )
        .await
        .check_status(StatusCode::NOT_FOUND);
    for recipients in [
        json!([]),
        json!(["valid@example.test", "bad..dots@example.test"]),
        json!([
            "valid@example.test",
            "victim@example.test\r\nBcc: other@example.test"
        ]),
    ] {
        th.client
            .post(
                &format!("{path}/mail"),
                &json!({"token":link.token,"recipients":recipients}),
            )
            .await
            .check_status(StatusCode::BAD_REQUEST);
    }
    th.client
        .delete(&format!("{path}/{}", link.id))
        .await
        .check_no_content();
    th.client
        .post(&format!("{path}/mail"), &request)
        .await
        .check_status(StatusCode::NOT_FOUND);
    assert!(th.app.finish_background(Duration::from_secs(1)).await);
    assert!(smtp.messages().await.is_empty());
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn invitation_mail_sends_guest_links_and_stops_when_guests_are_disabled() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    let board = th
        .create_board(ROOT_WORKSPACE_ID, fvoci_model::ContainerVisibility::Open)
        .await;
    let path = format!("/w/{ROOT_WORKSPACE_ID}/invitations");
    let link: Invitation = th
        .client
        .post(&path, &json!({"role":"guest","containerId":board.id}))
        .await
        .created_json();
    let request = json!({"token":link.token,"recipients":["guest@example.test"],"language":"en"});
    th.client
        .post(&format!("{path}/mail"), &request)
        .await
        .check_no_content();
    let mail = smtp.received(1).await;
    assert!(mail[0].decoded.contains("used once"));
    assert!(!mail[0].decoded.contains(&board.id));
    assert_eq!(link.max_uses, Some(1));
    th.client
        .put(
            &format!("/w/{ROOT_WORKSPACE_ID}/guest-invitations"),
            &json!({"enabled":false}),
        )
        .await
        .check_status(StatusCode::OK);
    th.client
        .post(&format!("{path}/mail"), &request)
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn invitation_mail_requires_smtp_but_plain_links_still_work() {
    let th = Setup::native().start().await.init_basic().await;
    let path = format!("/w/{ROOT_WORKSPACE_ID}/invitations");
    let link: Invitation = th
        .client
        .post(&path, &json!({"role":"member"}))
        .await
        .created_json();
    th.client
        .post(
            &format!("{path}/mail"),
            &json!({"token":link.token,"recipients":["outside@example.test"]}),
        )
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.anonymous()
        .post("/invitations/preview", &json!({"token":link.token}))
        .await
        .check_status(StatusCode::OK);
    th.teardown().await;
}

#[tokio::test]
async fn invitation_mail_cannot_use_another_organizations_token() {
    // Several organizations are a PostgreSQL deployment; SQLite refuses that mode.
    if fvoci_store::testing::db_type_of_the_tests() != fvoci_store::DbType::Postgres {
        return;
    }
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| {
            "open".clone_into(&mut config.org_creation);
        })
        .start()
        .await
        .init_basic()
        .await;
    let mut other = th.anonymous();
    other.post("/register", &json!({"organizationName":"Another organization","username":"mail-other-org","email":"other-org@example.test","password":"Other-password"})).await.check_status(StatusCode::CREATED);
    other
        .login("mail-other-org", "Other-password")
        .await
        .check_no_content();
    let workspaces: Vec<fvoci_model::UserWorkspace> =
        other.get("/users/me/workspaces").await.ok_json();
    let own = &workspaces[0].id;
    let link: Invitation = other
        .post(&format!("/w/{own}/invitations"), &json!({"role":"member"}))
        .await
        .created_json();
    let request = json!({"token":link.token,"recipients":["outside@example.test"]});
    th.client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/invitations/mail"),
            &request,
        )
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.client
        .post(&format!("/w/{own}/invitations/mail"), &request)
        .await
        .check_status(StatusCode::NOT_FOUND);
    assert!(smtp.messages().await.is_empty());
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn invitation_mail_preserves_distinct_case_sensitive_mailboxes() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    let path = format!("/w/{ROOT_WORKSPACE_ID}/invitations");
    let link: Invitation = th
        .client
        .post(&path, &json!({"role":"member"}))
        .await
        .created_json();
    th.client
        .post(
            &format!("{path}/mail"),
            &json!({
                "token":link.token,
                "recipients":["Smith@example.test", "smith@example.test", " Smith@example.test ", "smith@EXAMPLE.test"]
            }),
        )
        .await
        .check_no_content();
    smtp.received(1).await;
    assert!(th.app.finish_background(Duration::from_secs(5)).await);
    let mut recipients: Vec<String> = smtp
        .messages()
        .await
        .into_iter()
        .map(|message| message.recipient)
        .collect();
    recipients.sort();
    assert_eq!(
        recipients,
        ["Smith@example.test", "smith@example.test"],
        "local-part case identifies a mailbox; a domain is the same in any case"
    );
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn invitation_mail_without_the_recipients_language_is_written_in_the_senders() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    th.client
        .patch("/users/me/settings", &json!({"language":"ko"}))
        .await
        .check_status(StatusCode::OK);
    th.client2
        .patch("/users/me/settings", &json!({"language":"ja"}))
        .await
        .check_status(StatusCode::OK);
    let path = format!("/w/{ROOT_WORKSPACE_ID}/invitations");
    let link: Invitation = th
        .client
        .post(&path, &json!({"role":"member"}))
        .await
        .created_json();
    th.client
        .post(
            &format!("{path}/mail"),
            &json!({"token":link.token,"recipients":["user2@sample.com","outside@example.test"],"language":"en"}),
        )
        .await
        .check_no_content();
    let messages = smtp.received(2).await;
    for message in &messages {
        assert!(
            message.decoded.contains("lang=\"ko\""),
            "{}: the sender's setting before the screen's language",
            message.recipient
        );
    }
    th.client
        .patch("/users/me/settings", &json!({"language":"auto"}))
        .await
        .check_status(StatusCode::OK);
    th.client
        .post(
            &format!("{path}/mail"),
            &json!({"token":link.token,"recipients":["user2@sample.com"],"language":"ko-KR"}),
        )
        .await
        .check_no_content();
    let messages = smtp.received(3).await;
    assert!(
        messages[2].decoded.contains("lang=\"ko\""),
        "with no setting of either, the screen's language"
    );
    th.teardown().await;
    smtp.stop().await;
}

/// The tokens of the invitation links in `message`, in the order they appear.
fn invite_tokens(message: &crate::fake_smtp::Received) -> Vec<String> {
    let mut tokens: Vec<String> = message
        .decoded
        .split("/invite/")
        .skip(1)
        .map(|rest| {
            rest.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                .collect()
        })
        .collect();
    tokens.dedup();
    tokens
}

#[tokio::test]
async fn guest_invitation_mail_gives_each_recipient_a_one_use_link_of_their_own() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    let board = th
        .create_board(ROOT_WORKSPACE_ID, fvoci_model::ContainerVisibility::Open)
        .await;
    let path = format!("/w/{ROOT_WORKSPACE_ID}/invitations");
    let link: Invitation = th
        .client
        .post(
            &path,
            &json!({"role":"guest","containerId":board.id,"containerRole":"commenter"}),
        )
        .await
        .created_json();
    let original = link.token.clone().expect("token");
    th.client
        .post(
            &format!("{path}/mail"),
            &json!({"token":original,"recipients":["g1@example.test","g2@example.test","g3@example.test"]}),
        )
        .await
        .check_no_content();
    let messages = smtp.received(3).await;
    assert!(th.app.finish_background(Duration::from_secs(10)).await);
    let mut tokens: Vec<String> = messages
        .iter()
        .map(|message| {
            let tokens = invite_tokens(message);
            assert_eq!(tokens.len(), 1, "one link in each message: {tokens:?}");
            tokens[0].clone()
        })
        .collect();
    tokens.sort();
    tokens.dedup();
    assert_eq!(tokens.len(), 3, "every recipient has a link of their own");
    assert!(
        !tokens.contains(&original),
        "the link that was made stays its maker's"
    );
    for token in tokens.iter().chain([&original]) {
        th.anonymous()
            .post("/invitations/preview", &json!({"token":token}))
            .await
            .check_status(StatusCode::OK);
    }
    let links: Vec<Invitation> = th.client.get(&path).await.ok_json();
    let made: Vec<&Invitation> = links
        .iter()
        .filter(|other| other.role == fvoci_model::WorkspaceRole::Guest && other.id != link.id)
        .collect();
    assert_eq!(made.len(), 3, "{links:?}");
    for other in made {
        assert_eq!(other.role, fvoci_model::WorkspaceRole::Guest);
        assert_eq!(other.container_id.as_deref(), Some(board.id.as_str()));
        assert_eq!(other.container_role, link.container_role);
        assert_eq!(other.max_uses, Some(1));
        assert_eq!(
            other.expires_at, link.expires_at,
            "the invitation's own end"
        );
        assert_eq!(other.uses, 0);
    }
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn a_guest_link_mailed_by_another_inviter_is_made_by_the_sender_and_gone_when_refused() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    let user1 = th.client.me().await.id;
    let board: fvoci_model::Container = th
        .client2
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/boards"),
            &json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": "Plans"}),
        )
        .await
        .created_json();
    th.client2
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/containers/{}/members", board.id),
            &json!({"userId": user1, "role": "admin"}),
        )
        .await
        .check_ok();
    let path = format!("/w/{ROOT_WORKSPACE_ID}/invitations");
    let link: Invitation = th
        .client2
        .post(&path, &json!({"role":"guest","containerId":board.id}))
        .await
        .created_json();
    assert_ne!(link.created_by, user1);
    smtp.reject_next(1);
    th.client
        .post(
            &format!("{path}/mail"),
            &json!({"token":link.token,"recipients":["refused@example.test","taken@example.test"]}),
        )
        .await
        .check_no_content();
    assert!(th.app.finish_background(Duration::from_secs(10)).await);
    let messages = smtp.messages().await;
    assert_eq!(messages.len(), 1, "one refused for good: {messages:?}");
    let links: Vec<Invitation> = th.client.get(&path).await.ok_json();
    let made: Vec<&Invitation> = links
        .iter()
        .filter(|other| {
            other.container_id.as_deref() == Some(board.id.as_str()) && other.id != link.id
        })
        .collect();
    assert_eq!(
        made.len(),
        1,
        "the link of the refused message went with it: {links:?}"
    );
    assert_eq!(made[0].created_by, user1, "who sent it made it");
    let token = invite_tokens(&messages[0]).pop().expect("a link");
    th.anonymous()
        .post("/invitations/preview", &json!({"token":token}))
        .await
        .check_status(StatusCode::OK);
    th.teardown().await;
    smtp.stop().await;
}

/// A guest link to a wiki is mailed as one to a board is: each recipient gets a
/// one-use link of their own to the wiki, and comes in by it onto the wiki.
#[tokio::test]
async fn a_guest_link_to_a_wiki_is_mailed_and_lets_its_recipient_onto_the_wiki() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    let wiki = th
        .client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/wikis"),
            &json!({"visibility": "private", "title": "Handbook"}),
        )
        .await
        .value()["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let path = format!("/w/{ROOT_WORKSPACE_ID}/invitations");
    let link: Invitation = th
        .client
        .post(&path, &json!({"role": "guest", "containerId": wiki}))
        .await
        .created_json();
    th.client
        .post(
            &format!("{path}/mail"),
            &json!({"token": link.token, "recipients": ["reader@example.test"]}),
        )
        .await
        .check_no_content();
    let messages = smtp.received(1).await;
    assert!(th.app.finish_background(Duration::from_secs(10)).await);
    let tokens = invite_tokens(&messages[0]);
    assert_eq!(tokens.len(), 1, "{tokens:?}");
    let accepted = th
        .anonymous()
        .post(
            "/invitations/accept",
            &json!({"token": tokens[0], "account": {
                "username": "mailed.reader",
                "email": "reader@example.test",
                "password": crate::common::PASSWORD,
            }}),
        )
        .await;
    accepted.check_ok();
    assert_eq!(accepted.value()["containerId"], json!(wiki));
    let reader = th.login("mailed.reader", crate::common::PASSWORD).await;
    reader
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/wikis/{wiki}/pages"))
        .await
        .check_ok();
    th.teardown().await;
    smtp.stop().await;
}

/// `deferred P2-1 & H-4`: recipient deduplication folds the entire address to lowercase
/// (`Outside@Example.Test` and `outside@example.test` send once), and exceeding
/// `invitation_mail_per_address_hour` silently skips the rate-limited address (still
/// returning `204` with no extra guest child link created in the store).
#[tokio::test]
async fn invitation_mail_dedups_full_address_case_insensitively_and_enforces_per_address_quota_silently()
 {
    let smtp = FakeSmtp::start_on(0).await;
    let mut settings = smtp.settings();
    settings.invitation_mail_per_address_hour = 2;
    let th = Setup::native()
        .smtp(settings)
        .start()
        .await
        .init_basic()
        .await;
    let board = th
        .create_board(ROOT_WORKSPACE_ID, fvoci_model::ContainerVisibility::Private)
        .await;
    let path = format!("/w/{ROOT_WORKSPACE_ID}/invitations");
    let guest_link: Invitation = th
        .client
        .post(&path, &json!({"role": "guest", "containerId": board.id}))
        .await
        .created_json();
    let token = guest_link.token.as_deref().expect("token");

    // 1st send: domain case-variant duplicates collapse to 1 message (`Target@Example.Test` == `Target@example.test`).
    th.client
        .post(
            &format!("{path}/mail"),
            &json!({"token": token, "recipients": ["Target@Example.Test", "Target@example.test"]}),
        )
        .await
        .check_no_content();
    smtp.received(1).await;
    assert!(
        th.app.finish_background(Duration::from_secs(5)).await,
        "1st mail background finished"
    );
    assert_eq!(
        smtp.messages().await.len(),
        1,
        "domain case-insensitive dedup"
    );

    // 2nd send: reaches hourly limit (2) for `target@example.test`.
    th.client
        .post(
            &format!("{path}/mail"),
            &json!({"token": token, "recipients": ["TARGET@example.test"]}),
        )
        .await
        .check_no_content();
    smtp.received(2).await;
    assert!(
        th.app.finish_background(Duration::from_secs(5)).await,
        "2nd mail background finished"
    );

    let links_before: Vec<Invitation> = th.client.get(&path).await.ok_json();

    // 3rd send: `target@example.test` is over quota and silently skipped (`Queued::pass`,
    // creating NO child guest invitation row), while `peer@example.test` still receives mail!
    th.client
        .post(
            &format!("{path}/mail"),
            &json!({"token": token, "recipients": ["target@example.test", "peer@example.test"]}),
        )
        .await
        .check_no_content();
    smtp.received(3).await;
    assert!(
        th.app.finish_background(Duration::from_secs(5)).await,
        "3rd mail background finished"
    );
    let all_msgs = smtp.messages().await;
    assert_eq!(
        all_msgs.len(),
        3,
        "rate-limited target skipped, peer delivered"
    );
    assert_eq!(
        all_msgs[2].recipient, "peer@example.test",
        "only peer received 3rd message"
    );

    let links_after: Vec<Invitation> = th.client.get(&path).await.ok_json();
    assert_eq!(
        links_after.len(),
        links_before.len() + 1,
        "only delivered peer got a one-time guest invitation row, none created for skipped target"
    );

    th.teardown().await;
    smtp.stop().await;
}
