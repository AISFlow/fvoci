//! The inbox as its reader sorts it: listed by whether it is read, by what it tells and
//! by whether it was put away; one marked unread again, put away and taken back, and
//! every one that was read put away at once; more of one put away brings it back; and
//! none of it reaches what is another's.

use std::time::Duration;

use fvoci_model::{Block, BlockType, ContainerVisibility, Notification, NotificationKind};
use fvoci_server::app::tend_notifications;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::blocks::delete;
use crate::common::{Client, Setup, TEST_WS, TestServer, UNKNOWN_ID};
use crate::notifications::{
    OWNER, board_with_owner, card_with, comment_on, give, mentioning, told, unread,
};

/// A page of what `client`'s user was told, as `query` takes it.
async fn listed(client: &Client, query: &str) -> Vec<Notification> {
    client
        .get(&format!("/notifications?{query}"))
        .await
        .ok_json()
}

fn ids(told: &[Notification]) -> Vec<&str> {
    told.iter().map(|one| one.id.as_str()).collect()
}

/// Puts away, takes back and marks unread one at a time; lists each tab; puts every one
/// read away at once; and refuses all of it of a notification of another.
#[tokio::test]
async fn the_reader_sorts_their_inbox() {
    let th = Setup::native().start().await.init_basic().await;
    let b = th.client2.me().await;
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    // Three mentions, of three cards, and a card given: four lines.
    let mut cards = Vec::new();
    for _ in 0..3 {
        let card = card_with(&th.client, &board.id, &Value::Null).await;
        comment_on(&th.client, &card, &mentioning(&[&b.id])).await;
        tend_notifications(&th.app).await;
        tokio::time::sleep(Duration::from_millis(2)).await;
        cards.push(card);
    }
    th.client
        .patch(
            &format!("{TEST_WS}/cards/{}", cards[0].id),
            &json!({"updatedProperties": {OWNER: b.id}}),
        )
        .await
        .check_ok();
    let all = told(&th, &th.client2).await;
    assert_eq!(all.len(), 4, "{all:?}");
    assert!(all.iter().all(|one| one.archived_at == 0), "{all:?}");
    let given = &all[0];
    assert_eq!(
        given.subject.as_ref().map(|subject| subject.kind),
        Some(NotificationKind::Assignment)
    );
    let mentions = &all[1..];
    assert_eq!(
        ids(&listed(&th.client2, "kind=assignment").await),
        [given.id.as_str()]
    );
    assert_eq!(
        ids(&listed(&th.client2, "kind=mention").await),
        ids(mentions)
    );

    // Put away unread: read with it, out of the inbox, into the archive.
    let oldest = &mentions[2];
    let archive = format!("/notifications/{}/archive", oldest.id);
    th.client2.post_empty(&archive).await.check_no_content();
    assert_eq!(ids(&told(&th, &th.client2).await), ids(&all[..3]));
    assert_eq!(unread(&th.client2).await, 3, "it is read");
    let archived = listed(&th.client2, "archived=true").await;
    assert_eq!(ids(&archived), [oldest.id.as_str()]);
    assert!(
        archived[0].read_at > 0 && archived[0].archived_at > 0,
        "{archived:?}"
    );
    // Again: it stays put away since when it was.
    tokio::time::sleep(Duration::from_millis(2)).await;
    th.client2.post_empty(&archive).await.check_no_content();
    assert_eq!(listed(&th.client2, "archived=true").await, archived);
    assert_eq!(
        listed(&th.client2, "archived=true&kind=assignment").await,
        []
    );

    // Marked unread: back in the inbox, and counted.
    th.client2
        .post_empty(&format!("/notifications/{}/unread", oldest.id))
        .await
        .check_no_content();
    assert_eq!(listed(&th.client2, "archived=true").await, []);
    assert_eq!(unread(&th.client2).await, 4);
    assert_eq!(listed(&th.client2, "unread=true").await.len(), 4);

    // Every one read put away at once: the unread stay.
    for one in [&mentions[0], &mentions[1]] {
        th.client2
            .post_empty(&format!("/notifications/{}/read", one.id))
            .await
            .check_no_content();
    }
    th.client2
        .post_empty("/notifications/archive")
        .await
        .check_no_content();
    let inbox = told(&th, &th.client2).await;
    assert_eq!(ids(&inbox), [given.id.as_str(), oldest.id.as_str()]);
    assert_eq!(ids(&listed(&th.client2, "unread=true").await), ids(&inbox));
    assert_eq!(listed(&th.client2, "archived=true").await.len(), 2);
    // Taken back: in the inbox, and read still.
    th.client2
        .delete(&format!("/notifications/{}/archive", mentions[0].id))
        .await
        .check_no_content();
    let back = told(&th, &th.client2).await;
    assert_eq!(back.len(), 3);
    assert!(
        back.iter()
            .any(|one| one.id == mentions[0].id && one.read_at > 0 && one.archived_at == 0),
        "{back:?}"
    );
    assert_eq!(unread(&th.client2).await, 2);

    refused_of_another(&th, &given.id).await;
    assert_eq!(told(&th, &th.client2).await, back, "nothing changed");
    th.teardown().await;
}

/// The routes of one of B's (`theirs`), of none and of no identifier, by A: 404 alike;
/// A's own put away at once, which reaches none of B's; a filter that is none, 400; and
/// no session, 401.
async fn refused_of_another(th: &TestServer, theirs: &str) {
    for id in [theirs, UNKNOWN_ID, "no-identifier"] {
        for path in [
            format!("/notifications/{id}/unread"),
            format!("/notifications/{id}/archive"),
        ] {
            th.client.post_empty(&path).await.check_not_found();
        }
        th.client
            .delete(&format!("/notifications/{id}/archive"))
            .await
            .check_not_found();
    }
    th.client
        .post_empty("/notifications/archive")
        .await
        .check_no_content();
    for query in ["kind=nothing", "unread=maybe", "archived=1"] {
        let answer = th.client2.get(&format!("/notifications?{query}")).await;
        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{query}");
    }
    th.anonymous()
        .post_empty("/notifications/archive")
        .await
        .check_unauthorized();
}

/// More of one put away, within the hour, brings it back into the inbox, unread; one
/// whose card is gone is put away as any, and is in no list of a kind, as it says none.
#[tokio::test]
async fn more_of_one_put_away_brings_it_back_and_one_that_says_nothing_is_put_away() {
    let th = Setup::native().start().await.init_basic().await;
    let b = th.client2.me().await;
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    let card = card_with(&th.client, &board.id, &Value::Null).await;
    comment_on(&th.client, &card, &mentioning(&[&b.id])).await;
    let first = told(&th, &th.client2).await;
    assert_eq!(first.len(), 1);
    th.client2
        .post_empty(&format!("/notifications/{}/archive", first[0].id))
        .await
        .check_no_content();
    assert_eq!(told(&th, &th.client2).await, []);

    comment_on(&th.client, &card, &mentioning(&[&b.id])).await;
    let again = told(&th, &th.client2).await;
    assert_eq!(ids(&again), [first[0].id.as_str()], "the same line");
    assert_eq!((again[0].read_at, again[0].archived_at), (0, 0));
    assert_eq!(listed(&th.client2, "archived=true").await, []);

    let gone = Block {
        id: card.id.clone(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        ..Block::default()
    };
    delete(&th.client, &gone).await;
    let unseen = told(&th, &th.client2).await;
    assert!(unseen[0].subject.is_none(), "{unseen:?}");
    assert_eq!(listed(&th.client2, "kind=mention").await, []);
    assert_eq!(listed(&th.client2, "unread=true").await.len(), 1);
    th.client2
        .post_empty(&format!("/notifications/{}/archive", unseen[0].id))
        .await
        .check_no_content();
    let archived = listed(&th.client2, "archived=true").await;
    assert_eq!(ids(&archived), [first[0].id.as_str()]);
    assert!(archived[0].subject.is_none());
    assert_eq!(unread(&th.client2).await, 0);
    th.teardown().await;
}
