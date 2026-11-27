//! What a person is told inside the application: a mention in a body (typed together,
//! or written whole) and in a comment, and a card given to them; that nobody is told
//! of their own doing, nor twice of the same; and the list as its reader reads it.

use std::time::Duration;

use fvoci_model::{
    Block, BlockType, Card, Container, ContainerVisibility, Notification, NotificationKind,
    SearchKind, UnreadNotifications,
};
use fvoci_server::app::tend_notifications;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::blocks::{block_path, child_of, delete, insert_one, undelete_path};
use crate::boards::{delete_member, new_board};
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer, UNKNOWN_ID};
use crate::rooms::{Peer, card_holds};

/// Seeds a container membership directly in the store so mention/comment/assignment tests
/// measure content notifications independently of HTTP `added_to_container` alerts.
pub(crate) async fn give(th: &TestServer, container_id: &str, user_id: &str, role: &str) {
    let member = fvoci_model::ContainerMember::new(
        container_id,
        user_id,
        role.parse().expect("a valid role"),
    );
    th.app
        .store()
        .save_member(&member)
        .await
        .expect("the membership is stored");
}

pub(crate) async fn give_in(
    th: &TestServer,
    _workspace_id: &str,
    container_id: &str,
    user_id: &str,
    role: &str,
) {
    give(th, container_id, user_id, role).await;
}

/// The identifier of the person property of [`board_with_owner`].
pub(crate) const OWNER: &str = "owner";

/// A body of one paragraph that mentions each of `users`.
pub(crate) fn mentioning(users: &[&str]) -> Value {
    let mentions: Vec<Value> = users
        .iter()
        .map(|user| json!({"type": "mention", "attrs": {"userId": user}}))
        .collect();
    json!({"type": "doc", "content": [{"type": "paragraph", "content": mentions}]})
}

/// A body of one paragraph of text, which mentions nobody.
pub(crate) fn plain(text: &str) -> Value {
    json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]},
    ]})
}

/// A board of the test workspace of `board_type`, made by the first client's user,
/// whose cards have a person property ([`OWNER`]).
pub(crate) async fn board_with_owner(
    th: &TestServer,
    board_type: ContainerVisibility,
) -> Container {
    th.client
        .create_board(&Container {
            card_properties: vec![fvoci_model::CardPropertyDefinition::new(
                OWNER,
                "Owner",
                fvoci_model::PropertyType::Person,
            )],
            ..new_board(TEST_WORKSPACE_ID, board_type, "Told")
        })
        .await
        .created_json()
}

/// A card of the board with `body`, made by `client`'s user.
pub(crate) async fn card_with(client: &Client, board_id: &str, body: &Value) -> Card {
    client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/cards"),
            &json!({"title": "The card", "body": body}),
        )
        .await
        .created_json()
}

/// A comment on the card with `body`, made by `client`'s user.
pub(crate) async fn comment_on(client: &Client, card: &Card, body: &Value) -> Block {
    let parent = Block {
        id: card.id.clone(),
        container_id: card.board_id.clone(),
        ..Block::default()
    };
    let mut comment = child_of(&parent, BlockType::Comment, "");
    comment.body = fvoci_model::Body::parse(body.clone()).expect("a body");
    insert_one(client, &comment).await
}

/// Writes the body of a card whole, as `client`'s user.
pub(crate) async fn write_body(client: &Client, card_id: &str, body: &Value) {
    crate::rooms::write_whole(client, card_id, body).await;
}

/// Works off whom the changes so far may be told to, and answers the first page of
/// what `client`'s user was told, the newest first.
pub(crate) async fn told(th: &TestServer, client: &Client) -> Vec<Notification> {
    tend_notifications(&th.app).await;
    client.get("/notifications").await.ok_json()
}

/// How many notifications `client`'s user has not read.
pub(crate) async fn unread(client: &Client) -> i64 {
    let unread: UnreadNotifications = client.get("/notifications/unread").await.ok_json();
    unread.count
}

/// The kinds of the notifications whose subject their reader sees, sorted, and how
/// many of them have none.
pub(crate) fn kinds(told: &[Notification]) -> (Vec<&'static str>, usize) {
    let mut kinds: Vec<&str> = told
        .iter()
        .filter_map(|told| told.subject.as_ref())
        .map(|subject| subject.kind.as_str())
        .collect();
    kinds.sort_unstable();
    let unseen = told.iter().filter(|told| told.subject.is_none()).count();
    (kinds, unseen)
}

/// Waits until the document of the card is written to it, with a body that mentions
/// `users` in this order, a paragraph each.
async fn card_mentions(th: &TestServer, card_id: &str, users: &[&str]) {
    let paragraphs: Vec<Value> = users
        .iter()
        .map(|user| {
            json!({"type": "paragraph", "content": [
                {"type": "mention", "attrs": {"userId": user}},
            ]})
        })
        .collect();
    card_holds(
        &th.client,
        card_id,
        &json!({"type": "doc", "content": paragraphs}),
    )
    .await;
}

/// The end of the first slice: A mentions B in a card's body, typing it into the
/// card's document, and in a comment, and gives B the card. B is told of the three,
/// the two mentions in one line, and A of none; what is taken out and put back by
/// whoever put it there tells B nothing more; and once B is off the board, B's list
/// still has its two lines, which say nothing.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the end of the slice as one story: told, told no more, and seen no more"
)]
async fn a_mention_typed_a_mention_in_a_comment_and_a_card_given_are_told_once() {
    let th = Setup::native()
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    let (a, b) = (th.client.me().await, th.client2.me().await);
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    let card = card_with(&th.client, &board.id, &Value::Null).await;

    // Typed into the document of the card, and written to the card by the server.
    let mut one = Peer::connect(&th.client, 101).await;
    one.joined(TEST_WORKSPACE_ID, &card.id).await;
    let update = one.editor().append_mention(&b.id);
    one.send_update(&update).await;
    one.text("docAck").await;
    card_mentions(&th, &card.id, &[&b.id]).await;

    let comment = comment_on(&th.client, &card, &mentioning(&[&b.id])).await;
    th.client
        .patch(
            &block_path(&board.id, &card.id),
            &json!({"updatedFields": {"properties": {OWNER: b.id}}}),
        )
        .await
        .check_no_content();

    // The two mentions are of one card within the hour: one notification of them.
    let theirs = told(&th, &th.client2).await;
    assert_eq!(
        kinds(&theirs),
        (vec!["assignment", "mention"], 0),
        "{theirs:?}"
    );
    for one_told in &theirs {
        let subject = one_told.subject.as_ref().expect("B reads the card");
        assert_eq!(
            (
                subject.actor_id.as_str(),
                subject.workspace_id.as_str(),
                subject.container_id.as_str(),
                subject.block_id.as_str(),
                subject.block_kind,
                subject.title.as_str(),
                subject.people,
            ),
            (
                a.id.as_str(),
                TEST_WORKSPACE_ID,
                board.id.as_str(),
                card.id.as_str(),
                SearchKind::Card,
                "The card",
                1,
            ),
            "{one_told:?}"
        );
        assert_eq!(one_told.read_at, 0);
    }
    let of_kind = |told: &[Notification], kind: NotificationKind| {
        told.iter()
            .find(|one| one.subject.as_ref().is_some_and(|s| s.kind == kind))
            .and_then(|one| one.subject.clone())
            .expect("one of the kind")
    };
    let mention = of_kind(&theirs, NotificationKind::Mention);
    assert_eq!(
        (mention.times, mention.comment_id.as_str()),
        (2, comment.id.as_str()),
        "twice, the latest in the comment"
    );
    assert_eq!(
        of_kind(&theirs, NotificationKind::Assignment).comment_id,
        "",
        "the card given is of the card itself"
    );
    assert_eq!(unread(&th.client2).await, 2);
    assert_eq!(told(&th, &th.client).await, [], "A did all of it");
    assert_eq!(unread(&th.client).await, 0);

    // Taken out and put back: in the document, in the comment, and in the property.
    let before = crate::metrics::scrape(&th).await;
    let update = one.editor().delete_last();
    one.send_update(&update).await;
    one.text("docAck").await;
    let emptied = tokio::time::Instant::now() + Duration::from_secs(20);
    while crate::collab::body_of(&crate::collab::card(&th.client, &card.id).await).is_some() {
        assert!(
            tokio::time::Instant::now() < emptied,
            "the card keeps its body"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let update = one.editor().append_mention(&b.id);
    one.send_update(&update).await;
    one.text("docAck").await;
    card_mentions(&th, &card.id, &[&b.id]).await;
    for body in [plain("nobody"), mentioning(&[&b.id])] {
        let read_at = crate::blocks::blocks_of(&th.client, &board.id)
            .await
            .into_iter()
            .find(|block| block.id == comment.id)
            .expect("the comment")
            .updated_at;
        th.client
            .patch(
                &block_path(&board.id, &comment.id),
                &json!({"body": body, "readAt": read_at}),
            )
            .await
            .check_no_content();
        tend_notifications(&th.app).await;
    }
    for owner in ["", b.id.as_str()] {
        th.client
            .patch(
                &block_path(&board.id, &card.id),
                &json!({"updatedFields": {"properties": {OWNER: owner}}}),
            )
            .await
            .check_no_content();
        tend_notifications(&th.app).await;
    }
    // Each taken out and put back by whoever put it there was counted already.
    let again = told(&th, &th.client2).await;
    assert_eq!(again, theirs, "nothing more, and nothing changed");
    let after = crate::metrics::scrape(&th).await;
    let settled = |outcome: &str| {
        let of = |body: &str| {
            crate::metrics::sample(body, "fvoci_notifications_total", &[("outcome", outcome)])
                .unwrap_or_default()
        };
        of(&after) - of(&before)
    };
    assert_eq!(
        (settled("counted"), settled("made"), settled("joined")),
        (3.0, 0.0, 0.0),
        "each put back was noted, and found counted already: {after}"
    );
    assert_eq!(
        th.app
            .housekeeping()
            .store()
            .waiting_notification_events()
            .await
            .expect("counted"),
        0,
        "what was noted again was worked off, and dropped"
    );

    // Off the board: the two are still B's, and say nothing.
    delete_member(&th.client, &board.id, &b.id)
        .await
        .check_no_content();
    let unseen = told(&th, &th.client2).await;
    assert_eq!(kinds(&unseen), (vec![], 2), "{unseen:?}");
    let listed = serde_json::to_value(&unseen).expect("JSON");
    for (now, before) in listed.as_array().expect("a list").iter().zip(&again) {
        assert_eq!(
            *now,
            json!({
                "id": before.id,
                "createdAt": before.created_at,
                "updatedAt": before.updated_at,
                "readAt": 0,
                "subject": null,
            }),
            "nothing but that there was one"
        );
    }
    assert_eq!(unread(&th.client2).await, 2, "unread until they are read");
    th.teardown().await;
}

/// Reading: one at a time and all at once, only one's own, and only what was made by
/// the time given; the list comes in pages, the newest first.
#[tokio::test]
async fn notifications_are_read_one_by_one_and_all_at_once_by_their_reader_alone() {
    let th = Setup::native().start().await.init_basic().await;
    let b = th.client2.me().await;
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "viewer").await;
    // Each of another card: what happens to one card within the hour is one.
    for _ in 0..3 {
        let card = card_with(&th.client, &board.id, &Value::Null).await;
        comment_on(&th.client, &card, &mentioning(&[&b.id])).await;
        tend_notifications(&th.app).await;
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let theirs = told(&th, &th.client2).await;
    assert_eq!(theirs.len(), 3);
    assert!(
        theirs
            .windows(2)
            .all(|pair| pair[0].updated_at > pair[1].updated_at),
        "the newest first: {theirs:?}"
    );
    let second_page: Vec<Notification> = th
        .client2
        .get("/notifications?page=1&perPage=2")
        .await
        .ok_json();
    assert_eq!(
        second_page,
        theirs[2..],
        "the third alone is on the second page"
    );
    th.client2
        .get("/notifications?perPage=0")
        .await
        .check_bad_request();
    th.anonymous()
        .get("/notifications")
        .await
        .check_unauthorized();
    th.anonymous()
        .get("/notifications/unread")
        .await
        .check_unauthorized();

    // Another's notification is none: A cannot read B's.
    let oldest = &theirs[2];
    for path in [
        format!("/notifications/{}/read", oldest.id),
        format!("/notifications/{UNKNOWN_ID}/read"),
        "/notifications/no-identifier/read".to_owned(),
    ] {
        th.client.post_empty(&path).await.check_not_found();
    }
    assert_eq!(unread(&th.client2).await, 3);

    th.client2
        .post_empty(&format!("/notifications/{}/read", oldest.id))
        .await
        .check_no_content();
    let read_at = told(&th, &th.client2).await[2].read_at;
    assert!(read_at > 0, "it is read");
    assert_eq!(unread(&th.client2).await, 2);
    // Read again: it stays read since when it was.
    tokio::time::sleep(Duration::from_millis(2)).await;
    th.client2
        .post_empty(&format!("/notifications/{}/read", oldest.id))
        .await
        .check_no_content();
    assert_eq!(told(&th, &th.client2).await[2].read_at, read_at);

    // All that were made up to the second: the newest stays unread.
    th.client2
        .post_empty(&format!(
            "/notifications/read?through={}",
            theirs[1].created_at
        ))
        .await
        .check_no_content();
    let now = told(&th, &th.client2).await;
    assert_eq!(
        now.iter().map(|told| told.read_at > 0).collect::<Vec<_>>(),
        [false, true, true],
        "{now:?}"
    );
    th.client
        .post_empty("/notifications/read")
        .await
        .check_no_content();
    assert_eq!(
        unread(&th.client2).await,
        1,
        "A read A's own, which are none"
    );
    th.client2
        .post_empty("/notifications/read")
        .await
        .check_no_content();
    assert_eq!(unread(&th.client2).await, 0);
    let bad = th
        .client2
        .post_empty("/notifications/read?through=soon")
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    th.teardown().await;
}

/// Types a mention of `user` into the document the peer holds, and waits for the
/// server to have it.
async fn type_mention(peer: &mut Peer, user: &str) {
    let update = peer.editor().append_mention(user);
    peer.send_update(&update).await;
    peer.text("docAck").await;
}

/// A body typed by several is written to its card by the last of them, and each name
/// in it is the doing of whoever typed it. A names B, a third and themself, and B
/// types after: B and the third are told by A's doing, though B wrote last, and A is
/// told nothing of their own, though another typed too.
#[tokio::test]
async fn a_mention_typed_by_one_of_several_is_told_as_theirs_and_ones_own_is_not() {
    let th = Setup::native().start().await.init_basic().await;
    let (a, b) = (th.client.me().await, th.client2.me().await);
    let c = th.add_user("third").await;
    let c_id = c.me().await.id;
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    for user in [&b.id, &c_id] {
        give(&th, &board.id, user, "editor").await;
    }
    let card = card_with(&th.client, &board.id, &Value::Null).await;
    let mut one = Peer::connect(&th.client, 101).await;
    let mut two = Peer::connect(&th.client2, 102).await;
    one.joined(TEST_WORKSPACE_ID, &card.id).await;
    two.joined(TEST_WORKSPACE_ID, &card.id).await;

    for user in [&b.id, &c_id, &a.id] {
        type_mention(&mut one, user).await;
    }
    two.settle().await;
    two.write_acked("typed by B").await;
    th.app
        .collab()
        .write_body(&th.app.housekeeping(), &card.id)
        .await
        .expect("the body is written to the card");
    assert_eq!(
        crate::collab::card(&th.client, &card.id).await.modified_by,
        b.id,
        "B wrote last, and is the writer of the body"
    );
    for (who, client) in [("B", &th.client2), ("the third", &c)] {
        let theirs = told(&th, client).await;
        assert_eq!(kinds(&theirs), (vec!["mention"], 0), "{who}: {theirs:?}");
        assert_eq!(
            theirs[0].subject.as_ref().expect("seen").actor_id,
            a.id,
            "{who} was named by A, whoever typed last"
        );
    }
    assert_eq!(
        told(&th, &th.client).await,
        [],
        "A named A, and is told nothing"
    );

    // B alone names B and A in another card: A is told, B is not.
    let before = told(&th, &th.client2).await;
    let alone = card_with(&th.client2, &board.id, &Value::Null).await;
    let mut two = Peer::connect(&th.client2, 103).await;
    two.joined(TEST_WORKSPACE_ID, &alone.id).await;
    for user in [&b.id, &a.id] {
        type_mention(&mut two, user).await;
    }
    th.app
        .collab()
        .write_body(&th.app.housekeeping(), &alone.id)
        .await
        .expect("the body is written to the card");
    assert_eq!(
        told(&th, &th.client2).await,
        before,
        "nothing of B's own doing"
    );
    let of_a = told(&th, &th.client).await;
    assert_eq!(kinds(&of_a), (vec!["mention"], 0), "{of_a:?}");
    let subject = of_a[0].subject.as_ref().expect("seen");
    assert_eq!(
        (
            subject.actor_id.as_str(),
            subject.block_id.as_str(),
            subject.kind
        ),
        (b.id.as_str(), alone.id.as_str(), NotificationKind::Mention)
    );
    th.teardown().await;
}

/// Makes a comment on the board naming `user_id` with `sourceBoardId`, verifying that
/// copied blocks suppress notifications and auto-subscriptions.
async fn made_from_another_board(th: &TestServer, board_id: &str, source_id: &str, user_id: &str) {
    let anchor = card_with(&th.client, board_id, &Value::Null).await;
    let parent = Block {
        id: anchor.id,
        container_id: board_id.to_owned(),
        ..Block::default()
    };
    let mut comment = child_of(&parent, BlockType::Comment, "Made from");
    comment.body = fvoci_model::Body::parse(mentioning(&[user_id])).expect("a body");
    let made: Vec<Block> = th
        .client
        .post(
            &format!(
                "{}?sourceBoardId={source_id}",
                crate::blocks::blocks_path(board_id)
            ),
            &[comment],
        )
        .await
        .created_json();
    assert_eq!(
        th.app
            .store()
            .get_subscribers_for_block(&made[0].id)
            .await
            .expect("the watchers are read"),
        [],
        "blocks made from another board's make nobody watch them"
    );
}

/// What is no person's own words tells nobody: a write that asks for it, a copy of a
/// card, a card that comes back from the trash, a version of a body brought back, and
/// a card that is a template of its board. And a write tells only whom it newly
/// names: who was named already is not told again by another change of the body.
#[tokio::test]
async fn copies_restores_and_quiet_writes_tell_nobody_and_a_body_only_whom_it_newly_names() {
    let th = Setup::native().start().await.init_basic().await;
    let b = th.client2.me().await;
    let c = th.add_user("third").await;
    let c_id = c.me().await.id;
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    for user in [&b.id, &c_id] {
        give(&th, &board.id, user, "editor").await;
    }
    let card = card_with(&th.client, &board.id, &mentioning(&[&b.id])).await;
    assert_eq!(kinds(&told(&th, &th.client2).await), (vec!["mention"], 0));
    th.client2
        .post_empty("/notifications/read")
        .await
        .check_no_content();
    let waiting = || async {
        th.app
            .housekeeping()
            .store()
            .waiting_notification_events()
            .await
            .expect("counted")
    };

    // Another change of the body, with B still in it and C new: C alone is told.
    write_body(&th.client, &card.id, &mentioning(&[&b.id, &c_id])).await;
    assert_eq!(kinds(&told(&th, &c).await), (vec!["mention"], 0));
    assert_eq!(unread(&th.client2).await, 0, "B was named already");

    // A copy of the card names whom the card names, and tells nobody.
    th.client
        .post_empty(&format!("{}/duplicate", block_path(&board.id, &card.id)))
        .await
        .check_created();
    assert_eq!(waiting().await, 0, "a copy notes nothing");

    // Blocks made from those of another board, as a board is from a template.
    let source = board_with_owner(&th, ContainerVisibility::Private).await;
    made_from_another_board(&th, &board.id, &source.id, &b.id).await;
    assert_eq!(
        waiting().await,
        0,
        "blocks made from another board's note nothing"
    );

    // Asked to tell nobody.
    let quiet = card_with(&th.client, &board.id, &Value::Null).await;
    let quietly = th
        .client
        .patch(
            &format!("{}?disableNotify=true", block_path(&board.id, &quiet.id)),
            &json!({"body": mentioning(&[&b.id]), "readAt": quiet.updated_at,
                    "updatedFields": {"properties": {OWNER: b.id}}}),
        )
        .await;
    assert_eq!(quietly.status, StatusCode::NO_CONTENT, "{}", quietly.text());
    assert_eq!(waiting().await, 0, "a quiet write notes nothing");

    // To the trash and back.
    let block = Block {
        id: card.id.clone(),
        container_id: board.id.clone(),
        ..Block::default()
    };
    delete(&th.client, &block).await;
    th.client
        .post_empty(&undelete_path(&board.id, &card.id))
        .await
        .check_ok();
    assert_eq!(waiting().await, 0, "what comes back notes nothing");

    // A version of the body brought back: the one before C was named and after.
    let versions: Value = th
        .client
        .get(&format!("{TEST_WS}/cards/{}/body-versions", card.id))
        .await
        .ok_json();
    let listed = versions.as_array().expect("versions");
    assert!(listed.len() >= 2, "{versions}");
    write_body(&th.client, &card.id, &plain("nobody")).await;
    tend_notifications(&th.app).await;
    let newest = listed[0]["version"].as_i64().expect("a number");
    th.client
        .post_empty(&format!(
            "{TEST_WS}/cards/{}/body-versions/{newest}/restore",
            card.id
        ))
        .await
        .check_ok();
    assert_eq!(waiting().await, 0, "a version brought back notes nothing");

    // A card that is a template of its board.
    th.client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Template", "isTemplate": true, "body": mentioning(&[&b.id]),
                    "properties": {OWNER: b.id}}),
        )
        .await
        .check_created();
    assert_eq!(waiting().await, 0, "a template notes nothing");

    assert_eq!(
        unread(&th.client2).await,
        0,
        "B was told once, at the start"
    );
    assert_eq!(told(&th, &c).await.len(), 1);
    th.teardown().await;
}

/// Leaving a board and coming back: what was B's is seen again. The membership of a
/// board is asked each time the list is read, and nothing is kept of what it said.
#[tokio::test]
async fn what_cannot_be_seen_is_seen_again_once_its_reader_is_let_back() {
    let th = Setup::native().start().await.init_basic().await;
    let b = th.client2.me().await;
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "viewer").await;
    card_with(&th.client, &board.id, &mentioning(&[&b.id])).await;
    assert_eq!(kinds(&told(&th, &th.client2).await), (vec!["mention"], 0));
    delete_member(&th.client, &board.id, &b.id)
        .await
        .check_no_content();
    assert_eq!(kinds(&told(&th, &th.client2).await), (vec![], 1));
    give(&th, &board.id, &b.id, "commenter").await;
    assert_eq!(kinds(&told(&th, &th.client2).await), (vec!["mention"], 0));
    th.teardown().await;
}

/// A server works off whom to tell by itself, every second: the tests above say when
/// (`tend_notifications`), and this one waits for the server.
#[tokio::test]
async fn a_server_tells_people_by_itself() {
    let th = Setup::native()
        .configure(|config| config.notifications_by_hand = false)
        .start()
        .await
        .init_basic()
        .await;
    let b = th.client2.me().await;
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "viewer").await;
    card_with(&th.client, &board.id, &mentioning(&[&b.id])).await;

    let waited = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let theirs: Vec<Notification> = th.client2.get("/notifications").await.ok_json();
            if !theirs.is_empty() {
                return theirs;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    let theirs = waited.expect("the server told B within half a minute");
    assert_eq!(kinds(&theirs), (vec!["mention"], 0), "{theirs:?}");
    th.teardown().await;
}

/// Two who each name the other in one body, written to the card once: each is told
/// by the other, and neither of their own.
#[tokio::test]
async fn two_who_name_each_other_in_one_body_are_each_told_by_the_other() {
    let th = Setup::native().start().await.init_basic().await;
    let (a, b) = (th.client.me().await, th.client2.me().await);
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    let card = card_with(&th.client, &board.id, &Value::Null).await;
    let mut one = Peer::connect(&th.client, 111).await;
    let mut two = Peer::connect(&th.client2, 112).await;
    one.joined(TEST_WORKSPACE_ID, &card.id).await;
    two.joined(TEST_WORKSPACE_ID, &card.id).await;

    type_mention(&mut one, &b.id).await;
    two.settle().await;
    type_mention(&mut two, &a.id).await;
    th.app
        .collab()
        .write_body(&th.app.housekeeping(), &card.id)
        .await
        .expect("the body is written to the card");
    for (who, client, by) in [("A", &th.client, &b.id), ("B", &th.client2, &a.id)] {
        let theirs = told(&th, client).await;
        assert_eq!(kinds(&theirs), (vec!["mention"], 0), "{who}: {theirs:?}");
        assert_eq!(
            &theirs[0].subject.as_ref().expect("seen").actor_id,
            by,
            "{who} was named by the other"
        );
    }
    th.teardown().await;
}

/// The log of a document is folded into its state every so many updates, and with it
/// goes who sent which. The body is written to its card before that, so that a name
/// typed before the fold is still the doing of who typed it: A names B, and B types
/// on until the log is folded.
#[tokio::test]
async fn who_typed_a_name_is_known_though_the_log_was_folded_since() {
    // B types faster than a person may: the limit of frames is not what is shown.
    let th = Setup::native()
        .configure(|config| config.collab.collab_frames_per_second = 1_000_000)
        .start()
        .await
        .init_basic()
        .await;
    let (a, b) = (th.client.me().await, th.client2.me().await);
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    let card = card_with(&th.client, &board.id, &Value::Null).await;
    let mut one = Peer::connect(&th.client, 121).await;
    let mut two = Peer::connect(&th.client2, 122).await;
    one.joined(TEST_WORKSPACE_ID, &card.id).await;
    two.joined(TEST_WORKSPACE_ID, &card.id).await;

    type_mention(&mut one, &b.id).await;
    two.settle().await;
    for n in 0..fvoci_server::collab::FOLD_AFTER {
        two.write_acked(&format!("{n} ")).await;
    }
    th.app
        .collab()
        .write_body(&th.app.housekeeping(), &card.id)
        .await
        .expect("the body is written to the card");
    assert_eq!(
        crate::collab::card(&th.client, &card.id).await.modified_by,
        b.id,
        "B typed last"
    );
    let theirs = told(&th, &th.client2).await;
    assert_eq!(kinds(&theirs), (vec!["mention"], 0), "{theirs:?}");
    assert_eq!(theirs[0].subject.as_ref().expect("seen").actor_id, a.id);
    th.teardown().await;
}
