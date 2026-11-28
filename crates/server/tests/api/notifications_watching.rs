//! What is told of what people watch, what they switched off, and each person's
//! channel: who writes a card or a page, comments on it or is given it watches it and
//! is told of its new comments until they stop; a kind switched off is not told; and
//! every connection of a session hears that its user's notifications changed, of the
//! organizations that session opens.

use std::time::Duration;

use fvoci_model::{ContainerVisibility, Notification, NotificationKind, SearchKind};
use fvoci_server::app::tend_notifications;
use serde_json::{Value, json};

use crate::common::{Client, Setup, TEST_WS};
use crate::notifications::{
    OWNER, board_with_owner, card_with, comment_on, give, kinds, mentioning, plain, told,
};
use crate::search_pages::{new_page, new_wiki};
use crate::websocket::{Event, Socket};

/// Ends the watching of `block_id` by `client`'s user.
async fn stop_watching(client: &Client, block_id: &str) {
    let user_id = client.me().await.id;
    client
        .delete(&format!("{TEST_WS}/subscriptions/{block_id}/{user_id}"))
        .await
        .check_no_content();
}

/// The one notification of `kind` among `told`.
fn of_kind(told: &[Notification], kind: NotificationKind) -> Option<&Notification> {
    told.iter()
        .find(|one| one.subject.as_ref().is_some_and(|s| s.kind == kind))
}

/// Whether `socket` hears `notificationsChanged` within `wait`, other messages and
/// pings passed over.
async fn hears_notifications(socket: &mut Socket, wait: Duration) -> bool {
    loop {
        match socket.next(wait).await {
            Event::Json(message) if message["type"] == "notificationsChanged" => {
                assert_eq!(
                    message,
                    json!({"type": "notificationsChanged"}),
                    "nothing else"
                );
                return true;
            }
            Event::Json(_) | Event::Ping => {}
            Event::Closed | Event::Nothing => return false,
        }
    }
}

/// How long a connection is given to hear what it was told before the test asks.
const HEARD: Duration = Duration::from_secs(5);
/// How long a connection is listened to for what it is not to hear: what is told is
/// queued by the time the work that tells it ends.
const NOT_HEARD: Duration = Duration::from_millis(500);

/// The writer of a card watches it, and so does who comments on it and who is given
/// it: each is told of a comment somebody else writes, by its writer, at that comment,
/// and nobody of their own. One who stops watching is told no more, and one who is
/// named in a comment is told of the mention alone.
#[tokio::test]
async fn whoever_writes_comments_on_or_is_given_a_card_is_told_of_its_comments_until_they_stop() {
    let th = Setup::native().start().await.init_basic().await;

    let b = th.client2.me().await;
    let c_client = th.add_user("third").await;
    let c = c_client.me().await;
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    for user in [&b.id, &c.id] {
        give(&th, &board.id, user, "editor").await;
    }
    let card = card_with(&th.client, &board.id, &Value::Null).await;

    let by_b = comment_on(&th.client2, &card, &plain("first")).await;
    let to_a = told(&th, &th.client).await;
    assert_eq!(kinds(&to_a), (vec!["comment"], 0), "{to_a:?}");
    let subject = to_a[0].subject.as_ref().expect("A reads the card");
    assert_eq!(
        (
            subject.actor_id.as_str(),
            subject.block_id.as_str(),
            subject.comment_id.as_str(),
            subject.block_kind,
            subject.times,
        ),
        (
            b.id.as_str(),
            card.id.as_str(),
            by_b.id.as_str(),
            SearchKind::Card,
            1
        ),
    );
    assert_eq!(told(&th, &th.client2).await, [], "nobody of their own");

    // A comments: B watches now, as who commented. C is given the card, and watches.
    comment_on(&th.client, &card, &plain("second")).await;
    th.client
        .patch(
            &format!("{TEST_WS}/cards/{}", card.id),
            &json!({"updatedProperties": {OWNER: c.id}}),
        )
        .await
        .check_ok();
    assert_eq!(kinds(&told(&th, &th.client2).await), (vec!["comment"], 0));
    assert_eq!(kinds(&told(&th, &c_client).await), (vec!["assignment"], 0));

    // A stops watching; C comments and names B: A is told nothing more, B of the
    // mention alone.
    stop_watching(&th.client, &card.id).await;
    let by_c = comment_on(&c_client, &card, &mentioning(&[&b.id])).await;
    let to_a_now = told(&th, &th.client).await;
    assert_eq!(to_a_now, to_a, "A watches no more");
    let to_b = told(&th, &th.client2).await;
    assert_eq!(kinds(&to_b), (vec!["comment", "mention"], 0), "{to_b:?}");
    let comment = of_kind(&to_b, NotificationKind::Comment).expect("the comment");
    assert_eq!(
        comment.subject.as_ref().map(|s| s.times),
        Some(1),
        "the comment C wrote is told as a mention: {to_b:?}"
    );
    let mention = of_kind(&to_b, NotificationKind::Mention).expect("the mention");
    assert_eq!(
        mention.subject.as_ref().map(|s| s.comment_id.as_str()),
        Some(by_c.id.as_str())
    );

    // B comments again: C is told, as one who was given the card; one notification of
    // the comments on the card within the hour, the latest by B.
    let again = comment_on(&th.client2, &card, &plain("third")).await;
    let to_c = told(&th, &c_client).await;
    let on_card = of_kind(&to_c, NotificationKind::Comment)
        .and_then(|one| one.subject.as_ref())
        .expect("C is told of the comment");
    assert_eq!(
        (on_card.actor_id.as_str(), on_card.comment_id.as_str()),
        (b.id.as_str(), again.id.as_str())
    );

    // A starts watching again by commenting.
    comment_on(&th.client, &card, &plain("fourth")).await;
    comment_on(&th.client2, &card, &plain("fifth")).await;
    let to_a_last = told(&th, &th.client).await;
    let line = of_kind(&to_a_last, NotificationKind::Comment)
        .and_then(|one| one.subject.as_ref())
        .expect("told");
    assert_eq!((line.times, line.people), (2, 1), "{to_a_last:?}");
    th.teardown().await;
}

/// Who writes a page watches it and is told of its comments, as of a card's; stopped,
/// they are told nothing, and they may watch it again.
#[tokio::test]
async fn the_writer_of_a_page_is_told_of_its_comments() {
    let th = Setup::native().start().await.init_basic().await;
    let b = th.client2.me().await;
    let wiki = new_wiki(&th.client).await;
    crate::notifications::give_in(
        &th,
        crate::common::TEST_WORKSPACE_ID,
        &wiki,
        &b.id,
        "commenter",
    )
    .await;
    let page = new_page(&th.client, &wiki, "", "A page", "text").await;
    let made = th
        .client2
        .post(
            &format!("{TEST_WS}/wikis/{wiki}/pages/{}/comments", page.id),
            &json!({"body": plain("on the page")}),
        )
        .await;
    made.check_created();
    let comment_id = made.value()["id"].as_str().expect("an id").to_owned();
    let to_a = told(&th, &th.client).await;
    assert_eq!(kinds(&to_a), (vec!["comment"], 0), "{to_a:?}");
    let subject = to_a[0].subject.as_ref().expect("read");
    assert_eq!(
        (
            subject.block_id.as_str(),
            subject.block_kind,
            subject.comment_id.as_str()
        ),
        (page.id.as_str(), SearchKind::Page, comment_id.as_str())
    );

    let comment_again = async || {
        th.client2
            .post(
                &format!("{TEST_WS}/wikis/{wiki}/pages/{}/comments", page.id),
                &json!({"body": plain("again")}),
            )
            .await
            .check_created();
    };
    stop_watching(&th.client, &page.id).await;
    comment_again().await;
    let stopped = told(&th, &th.client).await;
    assert_eq!(
        (
            stopped.len(),
            stopped
                .first()
                .and_then(|one| one.subject.as_ref())
                .map(|line| line.times)
        ),
        (1, Some(1)),
        "nothing more: {stopped:?}"
    );
    let a_id = th.client.me().await.id;
    th.client
        .post(
            &format!("{TEST_WS}/subscriptions"),
            &json!({"blockType": "page", "blockId": page.id, "userId": a_id}),
        )
        .await
        .check_created();
    comment_again().await;
    let again = told(&th, &th.client).await;
    assert_eq!(
        (
            again.len(),
            again
                .first()
                .and_then(|one| one.subject.as_ref())
                .map(|line| line.times)
        ),
        (1, Some(2)),
        "watched again, and told: {again:?}"
    );
    th.teardown().await;
}

/// A kind switched off in the settings is not told, and stays off; the others are told
/// as before, and what is off is not made to come later.
#[tokio::test]
async fn a_kind_switched_off_is_not_told_and_stays_off() {
    let th = Setup::native().start().await.init_basic().await;
    let b = th.client2.me().await;
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    let card = card_with(&th.client2, &board.id, &Value::Null).await;
    let settings: Value = th
        .client2
        .patch("/users/me/settings", &json!({"notifyComments": false}))
        .await
        .ok_json();
    assert_eq!(
        (
            &settings["notifyComments"],
            &settings["notifyMentions"],
            &settings["notifyAssignments"]
        ),
        (&json!(false), &json!(true), &json!(true))
    );
    comment_on(&th.client, &card, &plain("not told")).await;
    comment_on(&th.client, &card, &mentioning(&[&b.id])).await;
    assert_eq!(kinds(&told(&th, &th.client2).await), (vec!["mention"], 0));

    let kept: Value = th.client2.get("/users/me/settings").await.ok_json();
    assert_eq!(kept["notifyComments"], json!(false), "it stays off");
    th.client2
        .patch(
            "/users/me/settings",
            &json!({"notifyComments": null, "notifyMentions": false}),
        )
        .await
        .check_ok();
    let other = card_with(&th.client, &board.id, &mentioning(&[&b.id])).await;
    comment_on(&th.client, &card, &plain("told")).await;
    let now = told(&th, &th.client2).await;
    assert_eq!(kinds(&now), (vec!["comment", "mention"], 0), "{now:?}");
    assert!(
        now.iter()
            .filter_map(|one| one.subject.as_ref())
            .all(|subject| subject.block_id != other.id),
        "the mention in the other card is not told: {now:?}"
    );
    th.teardown().await;
}

/// Each connection of a session hears that its user's notifications changed, whatever
/// it follows: when they are told something and when they read; nobody else's does.
#[tokio::test]
async fn every_connection_of_the_user_hears_that_their_notifications_changed() {
    let th = Setup::native().start().await.init_basic().await;
    let b = th.client2.me().await;
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    let card = card_with(&th.client2, &board.id, &Value::Null).await;
    let mut tabs = [
        Socket::connect_as(&th.client2).await,
        Socket::connect_as(&th.client2).await,
    ];
    let mut of_a = Socket::connect_as(&th.client).await;

    comment_on(&th.client, &card, &plain("told")).await;
    tend_notifications(&th.app).await;
    for tab in &mut tabs {
        assert!(hears_notifications(tab, HEARD).await, "each tab of B");
    }
    assert!(!hears_notifications(&mut of_a, NOT_HEARD).await, "not A's");

    // Joined to the same notification: heard again.
    comment_on(&th.client, &card, &plain("again")).await;
    tend_notifications(&th.app).await;
    for tab in &mut tabs {
        assert!(
            hears_notifications(tab, HEARD).await,
            "joined: each tab of B"
        );
    }

    let listed = told(&th, &th.client2).await;
    th.client2
        .post(
            &format!("/notifications/{}/read", listed[0].id),
            &Value::Null,
        )
        .await
        .check_no_content();
    for tab in &mut tabs {
        assert!(hears_notifications(tab, HEARD).await, "read: each tab of B");
    }
    // Nothing left to read: nothing changes, and nothing is heard.
    th.client2
        .post("/notifications/read", &Value::Null)
        .await
        .check_no_content();
    assert!(!hears_notifications(&mut tabs[1], NOT_HEARD).await);
    th.teardown().await;
}

/// What an organization behind its provider tells somebody, and their reading it, is
/// heard by the connections of their sessions made through that provider, and by no
/// other; what another tells them is heard by every one.
#[tokio::test]
async fn a_connection_hears_only_of_what_its_session_opens() {
    use crate::fake_oidc::FakeProvider;
    use crate::notifications_isolation::open_page_mentioning;
    use crate::sso::{
        ADMIN_SUBJECT, Passwords, Two, require_sso, through_provider, tie, two_organizations,
    };
    use fvoci_store::DbType;
    use fvoci_store::testing::db_type_of_the_tests;

    if db_type_of_the_tests() != DbType::Postgres {
        // An SQLite database holds one organization.
        return;
    }
    let fake = FakeProvider::start().await;
    let Two {
        ts: th,
        ours,
        theirs,
        their_workspace,
    } = two_organizations(&fake).await;
    let user1 = th.client.me().await.id;
    let user2 = th.client2.me().await.id;
    // Noted before ours takes only its provider's sessions, and told after.
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &user2, "editor").await;
    card_with(&th.client2, &board.id, &mentioning(&[&user1])).await;
    tie(&th, &th.client, &fake, ADMIN_SUBJECT).await;
    let through_ours = through_provider(&th, &fake, &ours, ADMIN_SUBJECT).await;
    require_sso(&through_ours, &ours, &fake, Passwords::Refused)
        .await
        .check_ok();

    let mut with_password = Socket::connect_as(&th.client).await;
    let mut with_provider = Socket::connect_as(&through_ours).await;
    tend_notifications(&th.app).await;
    assert!(hears_notifications(&mut with_provider, HEARD).await);
    assert!(
        !hears_notifications(&mut with_password, NOT_HEARD).await,
        "the password's session hears nothing of ours"
    );
    // Read through the provider: heard by that session alone, as what it read is ours.
    let ours_told = told(&th, &through_ours).await;
    assert_eq!(ours_told.len(), 1, "{ours_told:?}");
    through_ours
        .post("/notifications/read", &Value::Null)
        .await
        .check_no_content();
    assert!(hears_notifications(&mut with_provider, HEARD).await, "read");
    assert!(
        !hears_notifications(&mut with_password, NOT_HEARD).await,
        "the password's session hears nothing of what was read of ours"
    );

    let their_store = th.app.store().for_org(&theirs).expect("an identifier");
    let author = their_author(&their_store).await;
    open_page_mentioning(&their_store, &their_workspace, &author, &[&user1]).await;
    tend_notifications(&th.app).await;
    for socket in [&mut with_password, &mut with_provider] {
        assert!(hears_notifications(socket, HEARD).await, "theirs, by both");
    }
    th.teardown().await;
    fake.stop();
}

/// A user of the other organization, who writes there.
async fn their_author(store: &fvoci_store::Store) -> String {
    let mut author = fvoci_model::User {
        id: fvoci_model::new_id(),
        username: "author.of.theirs".to_owned(),
        email: "author.of.theirs@sample.com".to_owned(),
        ..fvoci_model::User::default()
    };
    store
        .create_user(&mut author)
        .await
        .expect("their user is stored");
    author.id
}
