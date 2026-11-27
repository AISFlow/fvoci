//! Whom a write may be told to, and what a person was told: the list a write of a
//! block puts its rows on in its own transaction, what the worker takes off it, and
//! the bounds a person's notifications are kept within.

use fvoci_model::{
    Block, BlockType, Body, Container, ContainerVisibility, NotificationKind, Workspace, new_id,
};
use fvoci_store::{
    Making, NotificationEvent, NotificationLimits, NotificationReader, Paging, Store,
    StoredNotification, Told, WorkspaceTrashing,
};
use serde_json::json;

use crate::common;

/// A time by which every row is due.
const LATER: i64 = i64::MAX;
/// An hour, and no cap anybody reaches.
const LIMITS: NotificationLimits = NotificationLimits {
    same_within_millis: 3_600_000,
    most_made_by_one: 1000,
    made_within_millis: 3_600_000,
    most_of_a_person: 1000,
};

/// The identifier of a notification that was made.
fn made(told: Told) -> String {
    match told {
        Told::Made(id) => id,
        other => panic!("a notification was to be made: {other:?}"),
    }
}

fn mentioning(user_id: &str) -> Option<Body> {
    Body::parse(json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "mention", "attrs": {"userId": user_id}}]},
    ]}))
    .expect("a document")
}

/// A board of a new workspace of the store's organization, its maker, and somebody
/// else of the organization.
async fn board_and_two(store: &Store) -> (String, String, String) {
    let writer = common::user(store, &format!("w{}", &new_id()[24..])).await;
    let named = common::user(store, &format!("n{}", &new_id()[24..])).await;
    let workspace_id = common::workspace(store).await;
    let mut boards = common::create_test_boards(store, &workspace_id, &writer, 1).await;
    (boards.remove(0).id, writer, named)
}

fn card(board_id: &str, body: Option<Body>) -> Block {
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        block_type: BlockType::Card,
        title: "A card".to_owned(),
        body,
        ..Block::default()
    }
}

async fn events(store: &Store) -> Vec<NotificationEvent> {
    store
        .due_notification_events(LATER, 1000)
        .await
        .expect("the list is read")
}

/// The user, reading with a session made by a password.
fn reader(user_id: &str) -> NotificationReader<'_> {
    NotificationReader {
        user_id,
        making: Making::with_password(),
    }
}

async fn told(store: &Store, user_id: &str) -> Vec<StoredNotification> {
    let all = Paging {
        page: 0,
        per_page: std::num::NonZeroU32::new(1000).expect("a page size"),
    };
    store
        .notifications_of_user(reader(user_id), all)
        .await
        .expect("the notifications are read")
}

/// A write that names somebody puts one row on the list, with the write: a write
/// that fails leaves none. The same noted again before it was worked off moves the
/// row on, and whoever read it before takes nothing off and tells nobody.
#[tokio::test]
async fn a_write_notes_whom_it_names_with_itself_and_a_row_noted_again_is_not_taken_off() {
    let test = common::setup().await;
    let store = &test.store;
    let (board_id, writer, named) = board_and_two(store).await;

    // Two cards in one write, the second on no board: nothing is stored, nor noted.
    let mut both = [
        card(&board_id, mentioning(&named)),
        card(&new_id(), mentioning(&named)),
    ];
    store
        .insert_blocks(&mut both, &writer)
        .await
        .expect_err("a card on no board");
    assert_eq!(events(store).await, [], "nothing of a write that failed");

    let mut block = card(&board_id, mentioning(&named));
    store
        .insert_block(&mut block, &writer)
        .await
        .expect("the card is stored");
    let noted = events(store).await;
    assert_eq!(noted.len(), 1, "{noted:?}");
    let first = &noted[0];
    assert_eq!(
        (
            first.recipient_id.as_str(),
            first.kind,
            first.source_id.as_str(),
            first.block_id.as_str(),
            first.container_id.as_str(),
            first.actor_id.as_str(),
            first.attempts,
        ),
        (
            named.as_str(),
            NotificationKind::Mention,
            block.id.as_str(),
            block.id.as_str(),
            board_id.as_str(),
            writer.as_str(),
            0,
        )
    );
    // The writer names themself: nobody is told of their own doing.
    let mut own = card(&board_id, mentioning(&writer));
    store
        .insert_block(&mut own, &writer)
        .await
        .expect("the card is stored");
    assert_eq!(events(store).await.len(), 1, "nothing of one's own");

    // Out of the body and in again, before the row was worked off.
    for body in [None, mentioning(&named)] {
        let mut again = block.clone();
        again.body = body;
        store
            .insert_block(&mut again, &writer)
            .await
            .expect("the card is written again");
    }
    let again = events(store).await;
    assert_eq!(again.len(), 1, "still one row: {again:?}");
    assert!(again[0].seq > first.seq, "moved on: {again:?}");
    assert_eq!(
        store.tell_of(first, 10, LIMITS).await.expect("asked"),
        Told::NotedAgain,
        "read before it was noted again"
    );
    assert!(!store.drop_notification_event(first).await.expect("asked"));
    assert_eq!(events(store).await, again, "the row stays as it is");
    assert_eq!(told(store, &named).await, []);

    // A row that could not be worked off waits, and counts its tries.
    store
        .notification_event_failed(&again[0], 500)
        .await
        .expect("noted");
    let due_now = store.due_notification_events(499, 10).await.expect("read");
    assert_eq!(due_now, [], "not due before its time");
    let waited = store.due_notification_events(500, 10).await.expect("read");
    assert_eq!(waited.len(), 1);
    assert_eq!((waited[0].attempts, waited[0].seq), (1, again[0].seq));

    let id = made(store.tell_of(&waited[0], 1000, LIMITS).await.expect("told"));
    assert_eq!(events(store).await, [], "taken off with the telling");
    let theirs = told(store, &named).await;
    assert_eq!(theirs.len(), 1);
    assert_eq!(
        (
            theirs[0].id.as_str(),
            theirs[0].created_at,
            theirs[0].read_at
        ),
        (id.as_str(), 1000, 0)
    );
    test.teardown().await;
}

/// The row noted when `by` takes the mention of `named` out of the body of `card` and
/// puts it back.
async fn mentioned_again(store: &Store, card: &Block, named: &str, by: &str) -> NotificationEvent {
    for body in [None, mentioning(named)] {
        let mut again = card.clone();
        again.body = body;
        store
            .insert_block(&mut again, by)
            .await
            .expect("the card is written again");
    }
    let noted = events(store).await;
    assert_eq!(noted.len(), 1, "{noted:?}");
    noted.into_iter().next().expect("one row")
}

/// What happens to one card, of one kind, within an hour of the first of it joins the
/// first's line: counted, by the latest, and unread again; the same person doing the
/// same in the same place again (a mention taken out and put back) is counted once.
/// After the hour, or of another card, it is a line of its own.
#[tokio::test]
async fn what_happens_to_one_card_within_the_hour_is_one_line_that_counts_it() {
    let test = common::setup().await;
    let store = &test.store;
    let (board_id, writer, named) = board_and_two(store).await;
    let third = common::user(store, &format!("t{}", &new_id()[24..])).await;
    let mut block = card(&board_id, mentioning(&named));
    store
        .insert_block(&mut block, &writer)
        .await
        .expect("the card is stored");
    let event = events(store).await.remove(0);
    let hour = LIMITS.same_within_millis;
    let start = 10 * hour;
    let id = made(store.tell_of(&event, start, LIMITS).await.expect("told"));
    assert_eq!(
        store
            .read_notification(reader(&named), &id, start + 1)
            .await
            .expect("read"),
        Some(event.org_id.clone()),
        "read, and of the organization it was told in"
    );

    // Out of the body and in again, by the writer and then by a third.
    let noted_again = async |by: &str| mentioned_again(store, &block, &named, by).await;
    let again = noted_again(&writer).await;
    assert_eq!(
        store
            .tell_of(&again, start + 10, LIMITS)
            .await
            .expect("told"),
        Told::Counted(id.clone()),
        "the writer's mention was counted"
    );
    let line = told(store, &named).await;
    assert_eq!(
        (
            line.len(),
            line[0].times,
            line[0].updated_at,
            line[0].read_at
        ),
        (1, 1, start, start + 1),
        "as it was, and read: {line:?}"
    );
    let within = noted_again(&third).await;
    assert_eq!(
        store
            .tell_of(&within, start + hour - 1, LIMITS)
            .await
            .expect("told"),
        Told::Joined(id.clone()),
        "within the hour"
    );
    assert_eq!(events(store).await, [], "the rows are taken off");
    let line = told(store, &named).await;
    assert_eq!(line.len(), 1, "{line:?}");
    assert_eq!(
        (
            line[0].times,
            line[0].people,
            line[0].actor_id.as_str(),
            line[0].created_at,
            line[0].updated_at,
            line[0].read_at,
        ),
        (2, 2, third.as_str(), start, start + hour - 1, 0),
        "twice, by two, the latest by the third; unread again"
    );

    let after = noted_again(&writer).await;
    let next = made(
        store
            .tell_of(&after, start + hour, LIMITS)
            .await
            .expect("told"),
    );
    let mut other = card(&board_id, mentioning(&named));
    store
        .insert_block(&mut other, &writer)
        .await
        .expect("another card is stored");
    let of_other = events(store).await.remove(0);
    let another = made(
        store
            .tell_of(&of_other, start + hour, LIMITS)
            .await
            .expect("told"),
    );
    let ids: Vec<String> = told(store, &named)
        .await
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(ids.len(), 3, "an hour later, and another card: {ids:?}");
    assert!(ids.contains(&next) && ids.contains(&another));
    test.teardown().await;
}

/// One person makes so many lines for another within the hour and no more; past that,
/// what they do makes none, what somebody else does still does, and after the hour
/// theirs does again.
#[tokio::test]
async fn one_person_makes_so_many_lines_for_another_within_the_hour() {
    let test = common::setup().await;
    let store = &test.store;
    let (board_id, writer, named) = board_and_two(store).await;
    let third = common::user(store, &format!("t{}", &new_id()[24..])).await;
    let two = NotificationLimits {
        most_made_by_one: 2,
        ..LIMITS
    };
    let new_card_by = async |by: &str, at: i64| {
        let mut block = card(&board_id, mentioning(&named));
        store
            .insert_block(&mut block, by)
            .await
            .expect("the card is stored");
        let event = events(store).await.remove(0);
        store.tell_of(&event, at, two).await.expect("asked")
    };
    let hour = two.made_within_millis;
    for at in [hour, hour + 1] {
        made(new_card_by(&writer, at).await);
    }
    assert_eq!(new_card_by(&writer, hour + 2).await, Told::OverTheLimit);
    assert_eq!(events(store).await, [], "and the row is taken off");
    made(new_card_by(&third, hour + 3).await);
    made(new_card_by(&writer, 2 * hour + 1).await);
    assert_eq!(told(store, &named).await.len(), 4);
    test.teardown().await;
}

/// A new comment notes the watchers of its card but its writer and those it names; who
/// writes a card, comments on it or is given it watches it, and nobody watches by a
/// quiet write.
#[tokio::test]
async fn a_comment_notes_the_watchers_of_its_card_and_writing_makes_a_watcher() {
    let test = common::setup().await;
    let store = &test.store;
    let (board_id, writer, named) = board_and_two(store).await;
    let third = common::user(store, &format!("t{}", &new_id()[24..])).await;
    let watchers = async |block_id: &str| {
        let mut ids: Vec<String> = store
            .get_subscribers_for_block(block_id)
            .await
            .expect("read")
            .into_iter()
            .map(|subscriber| subscriber.user_id)
            .collect();
        ids.sort();
        ids
    };
    let mut quiet = card(&board_id, None);
    store
        .telling_nobody()
        .insert_block(&mut quiet, &writer)
        .await
        .expect("a card stored quietly");
    assert_eq!(watchers(&quiet.id).await, Vec::<String>::new());

    let mut block = card(&board_id, None);
    store
        .insert_block(&mut block, &writer)
        .await
        .expect("the card is stored");
    assert_eq!(
        watchers(&block.id).await,
        std::slice::from_ref(&writer),
        "its writer"
    );
    let comment_by = async |by: &str, body: Option<Body>| {
        let mut comment = Block {
            id: new_id(),
            container_id: board_id.clone(),
            parent_id: block.id.clone(),
            block_type: BlockType::Comment,
            body,
            ..Block::default()
        };
        store
            .insert_block(&mut comment, by)
            .await
            .expect("the comment is stored");
        comment
    };
    let first = comment_by(&named, None).await;
    let noted: Vec<(String, NotificationKind, String, String)> = events(store)
        .await
        .into_iter()
        .map(|e| (e.recipient_id, e.kind, e.source_id, e.actor_id))
        .collect();
    assert_eq!(
        noted,
        [(
            writer.clone(),
            NotificationKind::Comment,
            first.id.clone(),
            named.clone()
        )],
        "the writer of the card, by the one who commented"
    );
    let mut expected = vec![writer.clone(), named.clone()];
    expected.sort();
    assert_eq!(watchers(&block.id).await, expected, "and who commented");
    for event in events(store).await {
        assert!(
            store
                .drop_notification_event(&event)
                .await
                .expect("dropped")
        );
    }

    // The third comments and names the writer: the writer is told of the mention
    // alone, the other watcher of the comment.
    let second = comment_by(&third, mentioning(&writer)).await;
    let mut noted: Vec<(String, NotificationKind)> = events(store)
        .await
        .into_iter()
        .filter(|e| e.source_id == second.id)
        .map(|e| (e.recipient_id, e.kind))
        .collect();
    noted.sort();
    let mut expected = vec![
        (writer.clone(), NotificationKind::Mention),
        (named.clone(), NotificationKind::Comment),
    ];
    expected.sort();
    assert_eq!(noted, expected);
    test.teardown().await;
}

/// Stores a card of the board that names the user, by the board's writer, and tells
/// the user of it at `at`. The identifier of the notification.
async fn told_of_a_new_card(
    store: &Store,
    (board_id, writer): (&str, &str),
    user_id: &str,
    at: i64,
    limits: NotificationLimits,
) -> String {
    let mut block = card(board_id, mentioning(user_id));
    store
        .insert_block(&mut block, writer)
        .await
        .expect("the card is stored");
    let event = events(store).await.remove(0);
    made(store.tell_of(&event, at, limits).await.expect("told"))
}

/// A person has so many notifications and no more: past it the oldest go, those that
/// were read before those that were not. And those read long ago are forgotten.
#[tokio::test]
async fn a_person_keeps_the_newest_and_the_unread_and_what_was_read_long_ago_is_forgotten() {
    let test = common::setup().await;
    let store = &test.store;
    let (board_id, writer, named) = board_and_two(store).await;
    let few = NotificationLimits {
        most_of_a_person: 3,
        ..LIMITS
    };
    // Somebody else has two, the oldest of all and read: nothing of theirs goes
    // for what the other is told.
    let other = common::user(store, &format!("o{}", &new_id()[24..])).await;
    let org = store
        .tenant()
        .org_id()
        .expect("the store reaches one organization");
    for at in [1_i64, 2] {
        let id = told_of_a_new_card(store, (&board_id, &writer), &other, at, few).await;
        assert_eq!(
            store
                .read_notification(reader(&other), &id, at + 1)
                .await
                .expect("read")
                .as_deref(),
            Some(org.as_str())
        );
    }
    // Five, one after another, each of another card; the second and the fourth are
    // read as they come.
    let mut made = Vec::new();
    for at in 1..=5_i64 {
        let id = told_of_a_new_card(store, (&board_id, &writer), &named, at * 100, few).await;
        if at == 2 || at == 4 {
            assert_eq!(
                store
                    .read_notification(reader(&named), &id, at * 100 + 1)
                    .await
                    .expect("read")
                    .as_deref(),
                Some(org.as_str())
            );
        }
        made.push(id);
        let held: Vec<String> = told(store, &named)
            .await
            .into_iter()
            .map(|n| n.id)
            .collect();
        let expected: Vec<&String> = match at {
            1 => vec![&made[0]],
            2 => vec![&made[1], &made[0]],
            3 => vec![&made[2], &made[1], &made[0]],
            // Past three: the one that was read goes, older ones that were not stay.
            4 => vec![&made[3], &made[2], &made[0]],
            // The fourth was read, and goes before the first.
            _ => vec![&made[4], &made[2], &made[0]],
        };
        assert_eq!(held.iter().collect::<Vec<_>>(), expected, "after {at}");
    }

    assert_eq!(
        told(store, &other).await.len(),
        2,
        "the other's are as they were"
    );

    // Another's notification is none of theirs to read.
    assert_eq!(
        store
            .read_notification(reader(&writer), &made[4], 1000)
            .await
            .expect("asked"),
        None,
        "the writer has no such notification"
    );
    assert_eq!(
        store
            .unread_notifications_of_user(reader(&named))
            .await
            .expect("counted"),
        3
    );
    // All whose latest happened by 300 are read at 2000; the newest stays unread.
    assert_eq!(
        store
            .read_notifications_of_user(reader(&named), 300, 2000)
            .await
            .expect("read")
            .into_iter()
            .collect::<Vec<_>>(),
        [(org.clone(), 2)],
        "two, of the one organization"
    );
    // Read before a time: the other's two, read long ago; then the two read at 2000.
    let forgotten = async |before: i64| {
        store
            .forget_read_notifications(before)
            .await
            .expect("asked")
    };
    assert_eq!(forgotten(4).await, 2, "the other's, read at 2 and 3");
    assert_eq!(forgotten(2000).await, 0, "read at 2000, and not before it");
    assert_eq!(forgotten(2001).await, 2);
    let left = told(store, &named).await;
    assert_eq!(left.len(), 1, "what was not read is kept however old");
    assert_eq!(
        (left[0].id.as_str(), left[0].read_at),
        (made[4].as_str(), 0)
    );
    test.teardown().await;
}

/// A handle that tells nobody notes nothing; and a card given to somebody, by a
/// person property of its board, is noted as that, once for each person it takes.
#[tokio::test]
async fn a_card_given_is_noted_for_whom_it_takes_and_a_quiet_handle_notes_nothing() {
    let test = common::setup().await;
    let store = &test.store;
    let (_, writer, named) = board_and_two(store).await;
    let other = common::user(store, &format!("o{}", &new_id()[24..])).await;
    let card_properties = vec![
        fvoci_model::CardPropertyDefinition::new(
            "owner",
            "Owner",
            fvoci_model::PropertyType::Person,
        ),
        fvoci_model::CardPropertyDefinition::new(
            "helpers",
            "Helpers",
            fvoci_model::PropertyType::MultiPerson,
        ),
        fvoci_model::CardPropertyDefinition::new("note", "Note", fvoci_model::PropertyType::Text),
    ];
    let mut board = Container {
        id: new_id(),
        workspace_id: common::workspace(store).await,
        visibility: ContainerVisibility::Open,
        created_by: writer.clone(),
        card_properties,
        ..Container::default()
    };
    store
        .insert_container(&mut board, &writer)
        .await
        .expect("the board is stored");
    let board_id = board.id;

    let with = |properties: serde_json::Value| {
        let mut block = card(&board_id, None);
        block.fields.insert("properties".to_owned(), properties);
        block
    };
    // A text that is somebody's identifier gives the card to nobody.
    let mut block = with(json!({"owner": named, "helpers": [other, writer], "note": other}));
    store
        .telling_nobody()
        .insert_block(&mut block.clone(), &writer)
        .await
        .expect("the card is stored quietly");
    assert_eq!(events(store).await, [], "a quiet handle notes nothing");
    block.id = new_id();
    store
        .insert_block(&mut block, &writer)
        .await
        .expect("the card is stored");
    let mut noted: Vec<(String, &str)> = events(store)
        .await
        .into_iter()
        .map(|event| (event.recipient_id, event.kind.as_str()))
        .collect();
    noted.sort();
    let mut expected = vec![
        (named.clone(), NotificationKind::Assignment.as_str()),
        (other.clone(), NotificationKind::Assignment.as_str()),
    ];
    expected.sort();
    assert_eq!(noted, expected, "whom it takes, but for the writer");
    for event in events(store).await {
        assert!(
            store
                .drop_notification_event(&event)
                .await
                .expect("dropped")
        );
    }

    // Written again with the same people, and one fewer: nobody is new.
    let mut same = block.clone();
    same.fields.insert(
        "properties".to_owned(),
        json!({"owner": named, "helpers": [writer]}),
    );
    store
        .insert_block(&mut same, &writer)
        .await
        .expect("the card is written again");
    assert_eq!(events(store).await, [], "nobody it had not taken already");
    test.teardown().await;
}

/// The identifiers of the watchers of a block, sorted.
async fn watchers(store: &Store, block_id: &str) -> Vec<String> {
    let mut ids: Vec<String> = store
        .get_subscribers_for_block(block_id)
        .await
        .expect("the subscribers are read")
        .into_iter()
        .map(|subscriber| subscriber.user_id)
        .collect();
    ids.sort();
    ids
}

/// A board with an `owner` person property, created by `writer`.
async fn board_with_owner_property(store: &Store, writer: &str) -> Container {
    let mut board = Container {
        id: new_id(),
        workspace_id: common::workspace(store).await,
        visibility: ContainerVisibility::Open,
        created_by: writer.to_owned(),
        card_properties: vec![fvoci_model::CardPropertyDefinition::new(
            "owner",
            "Owner",
            fvoci_model::PropertyType::Person,
        )],
        ..Container::default()
    };
    store
        .insert_container(&mut board, writer)
        .await
        .expect("the board is stored");
    board
}

/// A quiet write, as its writer asked, notes nobody to be told, and makes its writer
/// alone a watcher of what it writes new: of a card, and of the card a comment is on.
/// Who is given a card by somebody else is no watcher of it until the server tells
/// them of it (`watch_given`); who gives it to themselves is one at once.
#[tokio::test]
async fn a_quiet_write_tells_nobody_and_its_writer_alone_watches_what_it_writes() {
    let test = common::setup().await;
    let store = &test.store;
    let (_, writer, named) = board_and_two(store).await;
    let other = common::user(store, &format!("o{}", &new_id()[24..])).await;
    let board = board_with_owner_property(store, &writer).await;
    let quietly = store.quietly();

    let mut copy = card(&board.id, mentioning(&named));
    copy.fields
        .insert("properties".to_owned(), json!({"owner": other}));
    quietly
        .insert_block(&mut copy, &writer)
        .await
        .expect("the card is stored quietly");
    assert_eq!(events(store).await, [], "nobody is noted");
    assert_eq!(
        watchers(store, &copy.id).await,
        std::slice::from_ref(&writer),
        "its writer"
    );

    let mut comment = Block {
        id: new_id(),
        container_id: board.id.clone(),
        parent_id: copy.id.clone(),
        block_type: BlockType::Comment,
        body: mentioning(&other),
        ..Block::default()
    };
    quietly
        .insert_block(&mut comment, &named)
        .await
        .expect("the comment is stored quietly");
    assert_eq!(events(store).await, [], "nobody is noted of the comment");
    let mut expected = vec![writer.clone(), named.clone()];
    expected.sort();
    assert_eq!(
        watchers(store, &copy.id).await,
        expected,
        "and who commented"
    );

    // Given to somebody new, quietly: they are no watcher, and noted of nothing.
    let mut given = copy.clone();
    given
        .fields
        .insert("properties".to_owned(), json!({"owner": named}));
    quietly
        .insert_block(&mut given, &writer)
        .await
        .expect("the card is given quietly");
    assert_eq!(events(store).await, []);

    // Given aloud by its writer to somebody else: noted, and a watcher once told.
    let mut aloud = card(&board.id, None);
    store
        .telling_nobody()
        .insert_block(&mut aloud, &writer)
        .await
        .expect("a card nobody watches");
    aloud
        .fields
        .insert("properties".to_owned(), json!({"owner": other}));
    store
        .insert_block(&mut aloud, &writer)
        .await
        .expect("the card is given");
    let noted: Vec<(String, NotificationKind)> = events(store)
        .await
        .into_iter()
        .map(|event| (event.recipient_id, event.kind))
        .collect();
    assert_eq!(noted, [(other.clone(), NotificationKind::Assignment)]);
    assert_eq!(watchers(store, &aloud.id).await, Vec::<String>::new());
    store
        .watch_given(&aloud.id, &other)
        .await
        .expect("made a watcher");
    assert_eq!(
        watchers(store, &aloud.id).await,
        std::slice::from_ref(&other)
    );

    // Given by its writer to themselves: a watcher at once, of nothing noted.
    for event in events(store).await {
        assert!(
            store
                .drop_notification_event(&event)
                .await
                .expect("dropped")
        );
    }
    aloud
        .fields
        .insert("properties".to_owned(), json!({"owner": writer}));
    store
        .insert_block(&mut aloud, &writer)
        .await
        .expect("the card is taken by its writer");
    assert_eq!(events(store).await, []);
    let mut expected = vec![writer.clone(), other.clone()];
    expected.sort();
    assert_eq!(watchers(store, &aloud.id).await, expected);
    test.teardown().await;
}

/// What goes for good takes with it what people were told of it, and what they were
/// still to be told: of a card deleted long enough ago when the trash is emptied, and
/// of every card of a board that goes. A card still in the trash keeps its
/// notifications, and so does one on another board.
#[tokio::test]
async fn what_goes_for_good_takes_what_was_told_of_it() {
    let test = common::setup().await;
    let store = &test.store;
    let (board_id, writer, named) = board_and_two(store).await;
    let workspace_id = common::workspace(store).await;
    let elsewhere = common::create_test_boards(store, &workspace_id, &writer, 1)
        .await
        .remove(0)
        .id;
    let mut told_of = Vec::new();
    for on in [&board_id, &board_id, &board_id, &elsewhere] {
        let mut block = card(on, mentioning(&named));
        store
            .insert_block(&mut block, &writer)
            .await
            .expect("the card is stored");
        let event = events(store).await.remove(0);
        made(store.tell_of(&event, 1, LIMITS).await.expect("told"));
        told_of.push(block);
    }
    let [purged, trashed, kept, other] = &told_of[..] else {
        unreachable!("four cards");
    };
    let mut waiting = card(&board_id, mentioning(&named));
    store
        .insert_block(&mut waiting, &writer)
        .await
        .expect("the card is stored");
    assert_eq!(events(store).await.len(), 1, "its row waits");
    let about = async || {
        let mut ids: Vec<String> = told(store, &named)
            .await
            .into_iter()
            .map(|told| told.block_id)
            .collect();
        ids.sort();
        ids
    };

    for card in [&waiting, purged] {
        common::delete_blocks(store, std::slice::from_ref(card), &writer).await;
    }
    let deleted_at = store
        .get_deleted_block(&purged.id)
        .await
        .expect("in the trash")
        .block
        .deleted_at
        .expect("deletion time");
    let now = || {
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after 1970");
        i64::try_from(since.as_millis()).expect("milliseconds")
    };
    while now() <= deleted_at {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    common::delete_blocks(store, std::slice::from_ref(trashed), &writer).await;
    store
        .purge_trash(deleted_at + 1)
        .await
        .expect("the trash is emptied of what went first");
    let mut expected = vec![trashed.id.clone(), kept.id.clone(), other.id.clone()];
    expected.sort();
    assert_eq!(about().await, expected, "what was purged told nothing more");
    assert_eq!(events(store).await, [], "nor is to be told");

    store
        .delete_container(&board_id, &writer)
        .await
        .expect("the board is deleted");
    assert_eq!(about().await, expected, "a board in the trash keeps them");
    store
        .purge_trash(i64::MAX)
        .await
        .expect("the trash is emptied");
    assert_eq!(
        about().await,
        std::slice::from_ref(&other.id),
        "but for the other board's"
    );
    test.teardown().await;
}

/// Purging a workspace removes its boards and the blocks they leave behind
/// (`remove_what_they_left`), forgetting both settled notifications and waiting outbox
/// rows of that workspace while leaving another workspace's notifications intact.
#[tokio::test]
async fn purging_a_workspace_forgets_what_was_told_of_its_boards() {
    let test = common::setup().await;
    let store = &test.store;
    let (kept_board, writer, named) = board_and_two(store).await;
    for statement in [
        format!("UPDATE organization_members SET role = 'admin' WHERE user_id = '{writer}'"),
        format!("UPDATE users SET is_admin = true WHERE id = '{writer}'"),
    ] {
        test.execute_unchecked(&statement)
            .await
            .expect("the writer administers the organization");
    }
    let doomed_ws = Workspace {
        id: new_id(),
        title: "Doomed".to_owned(),
        ..Workspace::default()
    };
    store
        .create_named_workspace(&doomed_ws, &writer)
        .await
        .expect("the workspace is made");
    let doomed_board = common::create_test_boards(store, &doomed_ws.id, &writer, 1)
        .await
        .remove(0)
        .id;

    let mut kept_card = card(&kept_board, mentioning(&named));
    store
        .insert_block(&mut kept_card, &writer)
        .await
        .expect("kept card");
    let event = events(store).await.remove(0);
    made(store.tell_of(&event, 1, LIMITS).await.expect("told"));

    let mut settled_card = card(&doomed_board, mentioning(&named));
    store
        .insert_block(&mut settled_card, &writer)
        .await
        .expect("doomed settled card");
    let event = events(store).await.remove(0);
    made(store.tell_of(&event, 1, LIMITS).await.expect("told"));

    let mut waiting_card = card(&doomed_board, mentioning(&named));
    store
        .insert_block(&mut waiting_card, &writer)
        .await
        .expect("doomed waiting card");
    assert_eq!(events(store).await.len(), 1, "waiting outbox row");

    assert_eq!(
        store
            .trash_workspace(&doomed_ws.id, &writer)
            .await
            .expect("trashed"),
        WorkspaceTrashing::Done
    );
    store
        .purge_workspace(&doomed_ws.id, i64::MAX)
        .await
        .expect("purged");

    let left: Vec<String> = told(store, &named)
        .await
        .into_iter()
        .map(|one| one.block_id)
        .collect();
    assert_eq!(
        left,
        std::slice::from_ref(&kept_card.id),
        "only the kept workspace's notification remains"
    );
    assert_eq!(events(store).await, [], "waiting outbox row is gone");
    test.teardown().await;
}
