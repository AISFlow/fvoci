//! The activity of a card: a line for its making and one for each change of its title,
//! a property, its parent or whether it is a template, the newest first; one person's
//! changes of one thing within five minutes in one line; five hundred lines a card; the
//! lines kept while the card is in the trash and gone with it for good; and the lines
//! of an erased account without who did them.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use fvoci_model::{
    Block, BlockPatch, BlockType, CardEvent, CardEventKind, CardMovesRequest,
    CardPropertyDefinition, Container, ContainerPatch, ContainerVisibility, DuplicateAs,
    NewBoardView, PropertyColor, PropertyOption, PropertyType, ViewType, new_id,
};
use fvoci_store::{ByAgent, Erasing, Paging, Store};
use serde_json::{Value, json};

use crate::account_erasure::{T0, erase, founder};
use crate::common::{self, TestStore};

/// Every line of a card, on one page.
const EVERY: Paging = Paging {
    page: 0,
    per_page: NonZeroU32::new(1000).expect("a page size"),
};

const MINUTE: i64 = 60 * 1000;

/// A clock the test moves, from [`T0`].
struct Hands(Arc<AtomicI64>);

impl Hands {
    fn on(store: &Store) -> Self {
        let now = Arc::new(AtomicI64::new(T0));
        let read = Arc::clone(&now);
        store
            .clock()
            .set_source(move || read.load(Ordering::SeqCst));
        Self(now)
    }

    fn forward(&self, millis: i64) {
        self.0.fetch_add(millis, Ordering::SeqCst);
    }
}

/// A board of a workspace both work in, with a property of text and one of a date.
async fn board(test: &TestStore, users: &[&str]) -> String {
    let workspace_id = common::workspace_of(test, users).await;
    let mut board = Container {
        id: new_id(),
        workspace_id,
        visibility: ContainerVisibility::Open,
        title: "board".to_owned(),
        card_properties: vec![
            CardPropertyDefinition::new("note", "Note", PropertyType::Text),
            CardPropertyDefinition::new("due", "Due", PropertyType::Date),
        ],
        ..Container::default()
    };
    test.insert_container(&mut board, users[0])
        .await
        .expect("the board is stored");
    board.id
}

async fn new_card(store: &Store, board_id: &str, title: &str, user_id: &str) -> Block {
    let mut card = Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        block_type: BlockType::Card,
        title: title.to_owned(),
        ..Block::default()
    };
    store
        .insert_block(&mut card, user_id)
        .await
        .expect("the card is stored");
    card
}

fn note(value: &str) -> BlockPatch {
    let mut patch = BlockPatch::default();
    patch
        .updated_fields
        .insert("properties".to_owned(), json!({"note": value}));
    patch
}

fn title(value: &str) -> BlockPatch {
    BlockPatch {
        title: Some(value.to_owned()),
        ..BlockPatch::default()
    }
}

async fn patch(store: &Store, card: &Block, patch: &BlockPatch, user_id: &str) {
    store
        .patch_block(&card.id, patch, user_id)
        .await
        .expect("the card is changed");
}

async fn lines(store: &Store, card: &Block) -> Vec<CardEvent> {
    store
        .card_events(&card.id, EVERY)
        .await
        .expect("the lines are read")
}

/// Each line as kind, property, before, after.
fn said(lines: &[CardEvent]) -> Vec<(CardEventKind, Option<&str>, &Value, &Value)> {
    lines
        .iter()
        .map(|line| {
            (
                line.kind,
                line.property_id.as_deref(),
                &line.before,
                &line.after,
            )
        })
        .collect()
}

#[tokio::test]
async fn a_card_lists_its_making_and_each_change_the_newest_first() {
    let test = common::setup().await;
    let hands = Hands::on(&test);
    let maker = common::user(&test, "maker").await;
    let board_id = board(&test, &[&maker]).await;
    let parent = new_card(&test, &board_id, "parent", &maker).await;
    hands.forward(MINUTE);
    let card = new_card(&test, &board_id, "first", &maker).await;
    hands.forward(MINUTE);
    patch(&test, &card, &title("second"), &maker).await;
    hands.forward(MINUTE);
    let mut moved = BlockPatch {
        parent_id: Some(parent.id.clone()),
        ..BlockPatch::default()
    };
    moved.updated_fields.insert(
        "properties".to_owned(),
        json!({"due": {"from": "2026-03-05"}}),
    );
    patch(&test, &card, &moved, &maker).await;
    hands.forward(MINUTE);
    // Its body and icon are no line: the versions of the body say what it was.
    let mut unrecorded = BlockPatch::default();
    unrecorded
        .updated_fields
        .insert("icon".to_owned(), json!("🌱"));
    patch(&test, &card, &unrecorded, &maker).await;

    let lines = lines(&test, &card).await;
    let null = Value::Null;
    assert_eq!(
        said(&lines),
        [
            (
                CardEventKind::Property,
                Some("due"),
                &null,
                &json!({"from": "2026-03-05"})
            ),
            (CardEventKind::Parent, None, &null, &json!(parent.id)),
            (
                CardEventKind::Title,
                None,
                &json!("first"),
                &json!("second")
            ),
            (CardEventKind::Created, None, &null, &null),
        ],
        "the newest first, and of one write the last written first"
    );
    assert!(
        lines
            .iter()
            .all(|line| line.actor_id.as_deref() == Some(maker.as_str())
                && line.actor_token_name.is_none())
    );
    assert_eq!(lines[3].created_at, T0 + MINUTE, "made when it was stored");
    assert_eq!(
        test.card_events(
            &card.id,
            Paging {
                page: 1,
                per_page: NonZeroU32::new(3).expect("a page size"),
            },
        )
        .await
        .expect("the second page is read")
        .iter()
        .map(|line| line.kind)
        .collect::<Vec<_>>(),
        [CardEventKind::Created],
        "a page after the first"
    );
    test.teardown().await;
}

#[tokio::test]
async fn one_persons_changes_of_one_thing_within_five_minutes_are_one_line() {
    let test = common::setup().await;
    let hands = Hands::on(&test);
    let typist = common::user(&test, "typist").await;
    let other = common::user(&test, "other").await;
    let board_id = board(&test, &[&typist, &other]).await;
    let card = new_card(&test, &board_id, "card", &typist).await;
    let property_lines = async || {
        let all = lines(&test, &card).await;
        all.into_iter()
            .filter(|line| line.kind == CardEventKind::Property)
            .collect::<Vec<_>>()
    };

    hands.forward(MINUTE);
    patch(&test, &card, &note("a"), &typist).await;
    hands.forward(2 * MINUTE);
    patch(&test, &card, &note("ab"), &typist).await;
    hands.forward(3 * MINUTE);
    patch(&test, &card, &note("abc"), &typist).await;
    let one = property_lines().await;
    assert_eq!(one.len(), 1, "joined within five minutes of the last");
    assert_eq!(
        (&one[0].before, &one[0].after),
        (&Value::Null, &json!("abc"))
    );
    assert_eq!(one[0].created_at, T0 + MINUTE, "when it began");
    assert_eq!(
        one[0].updated_at,
        T0 + 6 * MINUTE,
        "when it was last changed"
    );

    // Another person's change is a line of its own, and the first person's next one
    // does not join a line from before it.
    hands.forward(MINUTE);
    patch(&test, &card, &note("other's"), &other).await;
    hands.forward(MINUTE);
    patch(&test, &card, &note("mine again"), &typist).await;
    assert_eq!(property_lines().await.len(), 3);

    // Past five minutes, a new line.
    hands.forward(5 * MINUTE + 1);
    patch(&test, &card, &note("later"), &typist).await;
    let later = property_lines().await;
    assert_eq!(later.len(), 4);
    assert_eq!(
        (&later[0].before, &later[0].after),
        (&json!("mine again"), &json!("later"))
    );

    // Taken back to what it was within five minutes: the line goes.
    hands.forward(MINUTE);
    patch(&test, &card, &note("mine again"), &typist).await;
    assert_eq!(
        property_lines().await.len(),
        3,
        "a change taken back is none"
    );

    // A title is a thing apart from the property.
    patch(&test, &card, &title("renamed"), &typist).await;
    patch(&test, &card, &note("then"), &typist).await;
    let all = lines(&test, &card).await;
    assert_eq!(
        all.iter().map(|line| line.kind).collect::<Vec<_>>(),
        [
            CardEventKind::Property,
            CardEventKind::Title,
            CardEventKind::Property,
            CardEventKind::Property,
            CardEventKind::Property,
            CardEventKind::Created,
        ]
    );
    test.teardown().await;
}

#[tokio::test]
async fn an_agents_change_names_its_token_and_is_not_its_persons_own() {
    let test = common::setup().await;
    let _hands = Hands::on(&test);
    let owner = common::user(&test, "owner").await;
    let board_id = board(&test, &[&owner]).await;
    let card = new_card(&test, &board_id, "card", &owner).await;
    let agent = test.by_agent(ByAgent {
        token_id: &new_id(),
        token_name: "laptop agent",
        notify_self: true,
    });
    patch(&agent, &card, &note("by the agent"), &owner).await;
    patch(&test, &card, &note("by hand"), &owner).await;
    let lines = lines(&test, &card).await;
    assert_eq!(
        lines.len(),
        3,
        "the agent's line is joined by no change of the person"
    );
    assert_eq!(lines[1].actor_token_name.as_deref(), Some("laptop agent"));
    assert_eq!(lines[1].actor_id.as_deref(), Some(owner.as_str()));
    assert_eq!(lines[0].actor_token_name, None);
    test.teardown().await;
}

#[tokio::test]
async fn a_copy_is_made_and_says_nothing_of_the_changes_of_its_source() {
    let test = common::setup().await;
    let _hands = Hands::on(&test);
    let maker = common::user(&test, "maker").await;
    let board_id = board(&test, &[&maker]).await;
    let card = new_card(&test, &board_id, "card", &maker).await;
    patch(&test, &card, &note("a value"), &maker).await;
    let copies = common::duplicate_card(&test, &board_id, &card.id, &maker, DuplicateAs::Copy)
        .await
        .expect("the card is copied");
    let copy_lines = lines(&test, &copies[0]).await;
    assert_eq!(
        copy_lines.iter().map(|line| line.kind).collect::<Vec<_>>(),
        [CardEventKind::Created]
    );
    assert_eq!(
        lines(&test, &card).await.len(),
        2,
        "the source's lines are its own"
    );

    // What tells nobody (a write of what was said before) is no line.
    let quiet = test.telling_nobody();
    patch(&quiet, &card, &title("told nobody"), &maker).await;
    assert_eq!(lines(&test, &card).await.len(), 2);
    test.teardown().await;
}

#[tokio::test]
async fn a_card_keeps_its_five_hundred_newest_lines() {
    let test = common::setup().await;
    let hands = Hands::on(&test);
    let maker = common::user(&test, "maker").await;
    let board_id = board(&test, &[&maker]).await;
    hands.forward(1000 * MINUTE);
    let card = new_card(&test, &board_id, "card", &maker).await;
    // Five hundred older lines, as many changes long ago would have left.
    let rows: Vec<String> = (0..500)
        .map(|at| {
            format!(
                "('{}', '{board_id}', '{}', 'title', '\"a\"', '\"b\"', {}, {})",
                new_id(),
                card.id,
                T0 + at,
                T0 + at
            )
        })
        .collect();
    test.execute_unchecked(&format!(
        "INSERT INTO card_events (id, container_id, card_id, kind, before_value, \
         after_value, created_at, updated_at) VALUES {}",
        rows.join(", ")
    ))
    .await
    .expect("the old lines are written");
    assert_eq!(lines(&test, &card).await.len(), 501);

    patch(&test, &card, &title("one more"), &maker).await;
    let kept = lines(&test, &card).await;
    assert_eq!(kept.len(), 500, "the oldest went");
    assert_eq!(kept[0].kind, CardEventKind::Title, "the newest stays");
    assert_eq!(
        kept.iter().map(|line| line.updated_at).min(),
        Some(T0 + 2),
        "the two oldest went"
    );
    test.teardown().await;
}

#[tokio::test]
async fn the_lines_stay_in_the_trash_and_go_with_the_card_for_good() {
    let test = common::setup().await;
    let hands = Hands::on(&test);
    let maker = common::user(&test, "maker").await;
    let board_id = board(&test, &[&maker]).await;
    let card = new_card(&test, &board_id, "card", &maker).await;
    let kept = new_card(&test, &board_id, "kept", &maker).await;
    patch(&test, &card, &title("changed"), &maker).await;

    hands.forward(MINUTE);
    test.delete_block(&card.id, &maker)
        .await
        .expect("the card goes to the trash");
    assert_eq!(lines(&test, &card).await.len(), 2, "kept in the trash");
    hands.forward(MINUTE);
    test.undelete_block(&card.id, &maker)
        .await
        .expect("the card comes back");
    let back = lines(&test, &card).await;
    assert_eq!(back[0].kind, CardEventKind::Restored);
    assert_eq!(back[0].created_at, T0 + 2 * MINUTE);
    assert_eq!(back.len(), 3);

    hands.forward(MINUTE);
    test.delete_block(&card.id, &maker)
        .await
        .expect("the card goes to the trash again");
    hands.forward(MINUTE);
    test.purge_trash(T0 + 10 * MINUTE)
        .await
        .expect("the trash is emptied");
    assert_eq!(lines(&test, &card).await, [], "gone for good");
    assert_eq!(lines(&test, &kept).await.len(), 1, "another card's stay");

    // With its board.
    test.delete_container(&board_id, &maker)
        .await
        .expect("the board goes to the trash");
    assert_eq!(lines(&test, &kept).await.len(), 1, "kept in the trash");
    test.purge_trash(T0 + 10 * MINUTE)
        .await
        .expect("the trash is emptied");
    assert_eq!(lines(&test, &kept).await, [], "gone with its board");
    test.teardown().await;
}

#[tokio::test]
async fn the_lines_of_an_erased_account_stay_without_who_did_them() {
    let test = common::setup().await;
    let founder = founder(&test).await;
    let leaver = common::user(&test, "leaver").await;
    let board_id = board(&test, &[&founder, &leaver]).await;
    let card = new_card(&test, &board_id, "card", &founder).await;
    let agent = test.by_agent(ByAgent {
        token_id: &new_id(),
        token_name: "leaver's agent",
        notify_self: true,
    });
    patch(&agent, &card, &title("by the leaver"), &leaver).await;
    erase(&test, &leaver, Erasing::AtOnce, T0 + 1).await;
    let lines = lines(&test, &card).await;
    assert_eq!(lines.len(), 2, "the lines stay");
    assert_eq!(lines[0].kind, CardEventKind::Title);
    assert_eq!(
        (
            lines[0].actor_id.as_deref(),
            lines[0].actor_token_name.as_deref()
        ),
        (None, None)
    );
    assert_eq!(
        lines[1].actor_id.as_deref(),
        Some(founder.as_str()),
        "another's stays"
    );
    test.teardown().await;
}

#[tokio::test]
async fn a_card_moved_to_another_column_has_one_line_of_its_property() {
    let test = common::setup().await;
    let _hands = Hands::on(&test);
    let mover = common::user(&test, "mover").await;
    let board_id = board(&test, &[&mover]).await;
    let (todo, done) = (new_id(), new_id());
    test.patch_container(
        &board_id,
        &ContainerPatch {
            updated_card_properties: vec![CardPropertyDefinition {
                id: "status".to_owned(),
                name: "Status".to_owned(),
                property_type: PropertyType::Select,
                options: vec![
                    PropertyOption::new(todo.clone(), "Todo", PropertyColor::Gray),
                    PropertyOption::new(done.clone(), "Done", PropertyColor::Green),
                ],
            }],
            ..ContainerPatch::default()
        },
        &mover,
    )
    .await
    .expect("the board has a status");
    let view = test
        .create_board_view(
            &board_id,
            &NewBoardView {
                title: "Board".to_owned(),
                view_type: ViewType::Board,
                after: None,
                fields: None,
            },
            &mover,
        )
        .await
        .expect("the view is made")
        .view;
    let card = new_card(&test, &board_id, "card", &mover).await;
    let mut first_move = BlockPatch::default();
    first_move
        .updated_fields
        .insert("properties".to_owned(), json!({"status": todo}));
    patch(&test, &card, &first_move, &mover).await;
    test.move_cards_in_view(
        &board_id,
        &view.id,
        &CardMovesRequest {
            card_ids: vec![card.id.clone()],
            group_property_id: Some("status".to_owned()),
            group_value: Some(json!(done)),
            after_card_id: None,
        },
        &mover,
    )
    .await
    .expect("the card moves");
    let lines = lines(&test, &card).await;
    assert_eq!(
        said(&lines),
        [
            (
                CardEventKind::Property,
                Some("status"),
                &Value::Null,
                &json!(done)
            ),
            (CardEventKind::Created, None, &Value::Null, &Value::Null),
        ],
        "the move joins the line of the same person's change a moment before"
    );
    test.teardown().await;
}
