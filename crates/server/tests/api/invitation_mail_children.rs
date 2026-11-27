//! The guest links made for the recipients of a mailed guest link: they name the link
//! they were made like and the address they were mailed to, go with it while nobody
//! used them, go out after it was used itself, and go when their message was given up.

use std::time::Duration;

use fvoci_model::{Invitation, WorkspaceRole};
use reqwest::StatusCode;
use serde_json::json;

use crate::common::{ROOT_WORKSPACE_ID, Setup, TestServer};
use crate::fake_smtp::FakeSmtp;

pub(crate) fn links_path() -> String {
    format!("/w/{ROOT_WORKSPACE_ID}/invitations")
}

/// A new guest link of a new board, made by user1.
async fn guest_link(th: &TestServer) -> Invitation {
    let board = th
        .create_board(ROOT_WORKSPACE_ID, fvoci_model::ContainerVisibility::Open)
        .await;
    th.client
        .post(
            &links_path(),
            &json!({"role":"guest","containerId":board.id,"containerRole":"viewer"}),
        )
        .await
        .created_json()
}

async fn mail(th: &TestServer, link: &Invitation, recipients: &[&str]) {
    th.client
        .post(
            &format!("{}/mail", links_path()),
            &json!({"token":link.token,"recipients":recipients}),
        )
        .await
        .check_no_content();
}

/// The live links made like `parent`, by the address they were mailed to.
pub(crate) async fn children(th: &TestServer, parent: &Invitation) -> Vec<String> {
    let links: Vec<Invitation> = th.client.get(&links_path()).await.ok_json();
    let mut recipients: Vec<String> = links
        .into_iter()
        .filter(|link| link.parent_id.as_deref() == Some(parent.id.as_str()))
        .map(|link| {
            assert_eq!(link.role, WorkspaceRole::Guest, "a child is a guest link");
            link.recipient.expect("a child names its recipient")
        })
        .collect();
    recipients.sort();
    recipients
}

#[tokio::test]
async fn mailed_guest_links_name_their_parent_and_recipient_and_go_with_it_unused() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    let parent = guest_link(&th).await;
    mail(&th, &parent, &["one@example.test", "two@example.test"]).await;
    smtp.received(2).await;
    assert!(th.app.finish_background(Duration::from_secs(10)).await);
    assert_eq!(
        children(&th, &parent).await,
        ["one@example.test", "two@example.test"]
    );
    let links: Vec<Invitation> = th.client.get(&links_path()).await.ok_json();
    let listed_parent = links
        .iter()
        .find(|link| link.id == parent.id)
        .expect("the parent is listed");
    assert_eq!(
        (&listed_parent.parent_id, &listed_parent.recipient),
        (&None, &None)
    );

    // One child is used; revoking the parent takes the other one along.
    let messages = smtp.messages().await;
    let one = messages
        .iter()
        .find(|message| message.recipient == "one@example.test")
        .expect("one's message");
    let token = one
        .decoded
        .split("/invite/")
        .nth(1)
        .and_then(|rest| {
            rest.split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
                .next()
        })
        .expect("a link")
        .to_owned();
    th.anonymous()
        .post(
            "/invitations/accept",
            &json!({"token": token, "account": {"username":"one","email":"one@example.test","password":"One-password-1"}}),
        )
        .await
        .check_ok();
    th.client
        .delete(&format!("{}/{}", links_path(), parent.id))
        .await
        .check_no_content();
    assert_eq!(
        children(&th, &parent).await,
        Vec::<String>::new(),
        "the unused child went with its parent"
    );
    let members: Vec<fvoci_model::WorkspaceMember> = th
        .client
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/members"))
        .await
        .ok_json();
    assert!(
        members.iter().any(|member| member.username == "one"),
        "who used their link stays: {members:?}"
    );
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn a_used_guest_link_is_still_mailed_as_what_its_recipients_links_are_made_like() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    let parent = guest_link(&th).await;
    th.anonymous()
        .post(
            "/invitations/accept",
            &json!({"token": parent.token, "account": {"username":"first","email":"first@example.test","password":"First-password-1"}}),
        )
        .await
        .check_ok();
    mail(&th, &parent, &["late@example.test"]).await;
    smtp.received(1).await;
    assert!(th.app.finish_background(Duration::from_secs(10)).await);
    assert_eq!(children(&th, &parent).await, ["late@example.test"]);
    // The used link is listed while a child of it is out, and revoked with it.
    let links: Vec<Invitation> = th.client.get(&links_path()).await.ok_json();
    assert!(links.iter().any(|link| link.id == parent.id), "{links:?}");
    th.client
        .delete(&format!("{}/{}", links_path(), parent.id))
        .await
        .check_no_content();
    assert_eq!(children(&th, &parent).await, Vec::<String>::new());
    let links: Vec<Invitation> = th.client.get(&links_path()).await.ok_json();
    assert!(links.iter().all(|link| link.id != parent.id), "{links:?}");
    th.client
        .post(
            &format!("{}/mail", links_path()),
            &json!({"token":parent.token,"recipients":["after@example.test"]}),
        )
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn the_link_of_a_message_given_up_is_revoked_and_may_be_mailed_again() {
    let smtp = FakeSmtp::start_on(0).await;
    // Every attempt of the first message is deferred: it is given up.
    smtp.fail_next(3);
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    let parent = guest_link(&th).await;
    mail(&th, &parent, &["later@example.test"]).await;
    assert!(th.app.finish_background(Duration::from_secs(20)).await);
    assert_eq!(smtp.attempts(), 3);
    assert!(smtp.messages().await.is_empty());
    assert_eq!(
        children(&th, &parent).await,
        Vec::<String>::new(),
        "nobody holds the link of a message given up"
    );
    // Three deferrals in a row hold invitations back for a while; a message taken lets
    // them go again.
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email":"user1@sample.com"}),
        )
        .await
        .check_no_content();
    smtp.received(1).await;
    mail(&th, &parent, &["later@example.test"]).await;
    smtp.received(2).await;
    assert!(th.app.finish_background(Duration::from_secs(10)).await);
    assert_eq!(children(&th, &parent).await, ["later@example.test"]);
    let refused = th
        .client
        .post(
            &format!("{}/mail", links_path()),
            &json!({"token":"not-a-token","recipients":["x@example.test"]}),
        )
        .await;
    refused.check_status(StatusCode::NOT_FOUND);
    th.teardown().await;
    smtp.stop().await;
}
