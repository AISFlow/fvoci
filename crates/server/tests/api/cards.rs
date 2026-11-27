//! Integration and application-layer tests for cards (the card view of blocks of type
//! `card`), covering permissions, validation order and error messages, paging, malformed
//! fields, and websocket/webhook notifications.
//!
//! Each test runs against its own isolated server and real store.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use fvoci_model::{
    BLOCK_TITLE_MAX_CHARS, Block, BlockType, Card, CardPatch, Container, ContainerMember,
    ContainerVisibility, JsonMap, is_id, new_id,
};
use fvoci_server::app::ChangeNotification;
use fvoci_server::error::ErrorKind;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::common::TEST_WS;
use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID, TestServer, UNKNOWN_ID};

/// How long background notifications may take to finish.
const BACKGROUND: Duration = Duration::from_secs(10);

// -- helpers --------------------------------------------------------------------------

/// A private board of the first user (`th.client`, its administrator) with one more
/// member of each role. `th.client2` is no member.
pub(crate) struct Roles {
    pub(crate) board: Container,
    pub(crate) editor: Client,
    pub(crate) commenter: Client,
    pub(crate) viewer: Client,
}

pub(crate) async fn board_with_roles(th: &TestServer) -> Roles {
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let editor = th.add_user("editor").await;
    let commenter = th.add_user("commenter").await;
    let viewer = th.add_user("viewer").await;
    let members = [
        ContainerMember::new(
            &board.id,
            editor.me().await.id,
            fvoci_model::ContainerRole::Editor,
        ),
        ContainerMember::new(
            &board.id,
            commenter.me().await.id,
            fvoci_model::ContainerRole::Commenter,
        ),
        ContainerMember::new(
            &board.id,
            viewer.me().await.id,
            fvoci_model::ContainerRole::Viewer,
        ),
    ];
    for member in &members {
        th.client.add_member(member).await.check_ok();
    }
    Roles {
        board,
        editor,
        commenter,
        viewer,
    }
}

/// Checks that the answer is the error with this status and message.
#[track_caller]
pub(crate) fn check_error(response: &ApiResponse, status: StatusCode, message: &str) {
    response.check_error(status, message);
}

pub(crate) fn cards_path(board_id: &str) -> String {
    format!("{TEST_WS}/boards/{board_id}/cards")
}

fn card_path(card_id: &str) -> String {
    format!("{TEST_WS}/cards/{card_id}")
}

/// `CreateCard`, expecting success. Returns the card, which is stored as answered.
async fn create_card(client: &Client, board_id: &str, card: &impl serde::Serialize) -> Card {
    let created: Card = client
        .post(&cards_path(board_id), card)
        .await
        .created_json();
    let read: Card = client.get(&card_path(&created.id)).await.ok_json();
    assert_eq!(read, created, "the card is stored as answered");
    read
}

/// `GetCards`, expecting success.
async fn cards_of(client: &Client, board_id: &str, query: &str) -> Vec<Card> {
    client
        .get(&format!("{}{query}", cards_path(board_id)))
        .await
        .ok_json()
}

/// `MakeCardProps`: `count` properties with random names and values.
fn make_card_props(count: usize) -> JsonMap {
    (0..count).map(|_| (new_id(), json!(new_id()))).collect()
}

/// `CreateBoardAndCards`: an open board of the first user with `count` cards.
async fn create_board_and_cards(th: &TestServer, count: usize) -> (Container, Vec<Card>) {
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let mut cards = Vec::with_capacity(count);
    for number in 1..=count {
        let card = Card {
            title: format!("test card {number}"),
            icon: "😱".to_owned(),
            properties: make_card_props(5),
            ..Card::default()
        };
        let created: Card = th
            .client
            .post(
                &format!("{}?disableNotify=true", cards_path(&board.id)),
                &card,
            )
            .await
            .created_json();
        cards.push(created);
    }
    (board, cards)
}

/// Builds a test block with a freshly generated identifier and both timestamps set to 1.
pub(crate) fn new_block(board_id: &str, parent_id: &str, block_type: BlockType) -> Block {
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        parent_id: parent_id.to_owned(),
        created_at: 1,
        updated_at: 1,
        block_type,
        ..Block::default()
    }
}

/// Inserts one block through the API and returns it as the server answered.
pub(crate) async fn insert_block(client: &Client, block: &Block) -> Block {
    let mut inserted: Vec<Block> = client
        .insert_blocks(&block.container_id, std::slice::from_ref(block))
        .await
        .created_json();
    inserted.pop().expect("one block was inserted")
}

/// Inserts a card block with the given raw fields through the application store.
async fn insert_card_block(th: &TestServer, board_id: &str, fields: Value) -> Block {
    let mut block = Block {
        fields: serde_json::from_value(fields).expect("the fields are a JSON object"),
        ..new_block(board_id, "", BlockType::Card)
    };
    let user_id = th.client.me().await.id;
    th.app
        .insert_block(&mut block, &user_id, ChangeNotification::Suppress)
        .await
        .expect("card block is stored");
    block
}

fn title_patch(title: &str) -> CardPatch {
    CardPatch {
        title: Some(title.to_owned()),
        ..CardPatch::default()
    }
}

/// Number of fixture cards created for card listing tests, titled `0` to `24`, each with three texts.
const CARD_COUNT: usize = 25;

async fn get_cards_fixture(th: &TestServer) -> Container {
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let user_id = th.client.me().await.id;
    for number in 0..CARD_COUNT {
        let card = Card {
            board_id: board.id.clone(),
            created_by: user_id.clone(),
            modified_by: user_id.clone(),
            title: number.to_string(),
            ..Card::default()
        };
        let path = format!("{}?disableNotify=true", cards_path(&board.id));
        let created: Card = th.client.post(&path, &card).await.created_json();

        let texts: Vec<Block> = (0..3)
            .map(|text| Block {
                created_by: user_id.clone(),
                modified_by: user_id.clone(),
                title: format!("text {text} for card {number}"),
                ..new_block(&board.id, &created.id, BlockType::Attachment)
            })
            .collect();
        th.client
            .post(
                &format!("{TEST_WS}/boards/{}/blocks?disableNotify=true", board.id),
                &texts,
            )
            .await
            .check_created();
    }
    board
}

/// Extracts the numeric titles from the fixture cards.
fn card_numbers(cards: &[Card]) -> BTreeSet<usize> {
    cards
        .iter()
        .map(|card| card.title.parse().expect("the title is the card's number"))
        .collect()
}

// -- Card HTTP route tests --------------------------------------------------------------

/// Unauthenticated `POST /boards/{board_id}/cards` is rejected.
#[tokio::test]
async fn create_card_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let card = Card {
        title: "basic card".to_owned(),
        ..Card::default()
    };

    let response = th.anonymous().post(&cards_path(&board.id), &card).await;

    response.check_unauthorized();
    th.teardown().await;
}

/// Creating a valid card succeeds and returns the stored card.
#[tokio::test]
async fn create_card_good() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let card = Card {
        title: "test card 1".to_owned(),
        icon: "😱".to_owned(),
        ..Card::default()
    };

    let card_new = create_card(&th.client, &board.id, &card).await;

    assert_eq!(card_new.board_id, board.id, "the card is on the board");
    assert_eq!(card_new.title, "test card 1", "the title");
    assert_eq!(card_new.icon, "😱", "the icon");
    th.teardown().await;
}

/// Creating an invalid card returns a bad request error.
#[tokio::test]
async fn create_card_invalid_card() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let card = Card {
        title: "too many emoji's".to_owned(),
        icon: "😱😱😱😱".to_owned(),
        ..Card::default()
    };

    let response = th.client.post(&cards_path(&board.id), &card).await;

    check_error(
        &response,
        StatusCode::BAD_REQUEST,
        "an icon is at most one grapheme",
    );
    assert_eq!(
        cards_of(&th.client, &board.id, "").await,
        [],
        "nothing was created"
    );
    th.teardown().await;
}

/// Fetching all cards on a board returns all created cards.
#[tokio::test]
async fn get_cards_fetch_all_cards() {
    let th = Setup::native().start().await.init_basic().await;
    let board = get_cards_fixture(&th).await;

    let cards = cards_of(&th.client, &board.id, "?page=0&perPage=1000").await;

    assert_eq!(cards.len(), CARD_COUNT, "all cards and none of the texts");
    th.teardown().await;
}

/// Fetching cards with `page` and `per_page` returns the requested pages without overlap.
#[tokio::test]
async fn get_cards_fetch_with_pagination() {
    let th = Setup::native().start().await.init_basic().await;
    let board = get_cards_fixture(&th).await;
    let mut card_numbers_seen = BTreeSet::new();

    for (page, expected_count) in [(0, 10), (1, 10), (2, 5)] {
        let query = format!("?page={page}&perPage=10");
        let cards = cards_of(&th.client, &board.id, &query).await;
        assert_eq!(cards.len(), expected_count, "the cards of page {page}");
        card_numbers_seen.extend(card_numbers(&cards));
    }

    assert_eq!(
        card_numbers_seen,
        (0..CARD_COUNT).collect(),
        "the pages together are all cards"
    );
    th.teardown().await;
}

/// Unauthenticated `GET /boards/{board_id}/cards` is rejected.
#[tokio::test]
async fn get_cards_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let board = get_cards_fixture(&th).await;

    let response = th
        .anonymous()
        .get(&format!("{}?page=0&perPage=10", cards_path(&board.id)))
        .await;

    response.check_unauthorized();
    th.teardown().await;
}

/// Unauthenticated `PATCH /cards/{card_id}` is rejected.
#[tokio::test]
async fn patch_card_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let (_, cards) = create_board_and_cards(&th, 1).await;
    let card = &cards[0];

    let response = th
        .anonymous()
        .patch(&card_path(&card.id), &title_patch("another title"))
        .await;

    response.check_unauthorized();
    th.teardown().await;
}

/// Patching a valid card updates its fields and properties.
#[tokio::test]
async fn patch_card_good() {
    let th = Setup::native().start().await.init_basic().await;
    let (board, cards) = create_board_and_cards(&th, 1).await;
    let card = &cards[0];

    let updated_props: JsonMap = card
        .properties
        .keys()
        .map(|id| (id.clone(), json!(new_id())))
        .collect();
    let patch = CardPatch {
        title: Some("another title".to_owned()),
        icon: Some("🐿".to_owned()),
        updated_properties: updated_props.clone(),
        ..CardPatch::default()
    };

    let patched: Card = th
        .client
        .patch(&card_path(&card.id), &patch)
        .await
        .ok_json();

    assert_eq!(patched.board_id, board.id, "the card stays on its board");
    assert_eq!(patched.title, "another title", "the new title");
    assert_eq!(patched.icon, "🐿", "the new icon");
    assert_eq!(patched.properties, updated_props, "the new properties");
    th.teardown().await;
}

/// An invalid card patch (such as a multi-grapheme icon) returns a bad request.
#[tokio::test]
async fn patch_card_invalid_card_patch() {
    let th = Setup::native().start().await.init_basic().await;
    let (_, cards) = create_board_and_cards(&th, 1).await;
    let card = &cards[0];
    let patch = CardPatch {
        icon: Some("🐿🐿🐿".to_owned()),
        ..CardPatch::default()
    };

    let response = th.client.patch(&card_path(&card.id), &patch).await;

    check_error(
        &response,
        StatusCode::BAD_REQUEST,
        "an icon is at most one grapheme",
    );
    let unchanged: Card = th.client.get(&card_path(&card.id)).await.ok_json();
    assert_eq!(&unchanged, card, "the card is as it was");
    th.teardown().await;
}

/// Unauthenticated `GET /cards/{card_id}` is rejected.
#[tokio::test]
async fn get_card_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let (_, cards) = create_board_and_cards(&th, 1).await;

    let response = th.anonymous().get(&card_path(&cards[0].id)).await;

    response.check_unauthorized();
    th.teardown().await;
}

/// Fetching an existing card by ID succeeds.
#[tokio::test]
async fn get_card_good() {
    let th = Setup::native().start().await.init_basic().await;
    let (board, cards) = create_board_and_cards(&th, 1).await;
    let card = &cards[0];

    let fetched: Card = th.client.get(&card_path(&card.id)).await.ok_json();

    assert_eq!(fetched.board_id, board.id, "the card's board");
    assert_eq!(fetched.title, card.title, "the title");
    assert_eq!(fetched.icon, card.icon, "the icon");
    assert_eq!(fetched.properties, card.properties, "the properties");
    th.teardown().await;
}

// -- Application-layer card tests, through the real store ---------------------------------

/// Application-layer `create_card` stores and returns the new card.
#[tokio::test]
async fn app_create_card_success_scenario() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let user_id = th.stranger().await;
    let card = Card {
        board_id: board.id.clone(),
        created_by: user_id.clone(),
        modified_by: user_id.clone(),
        title: "test card".to_owned(),
        properties: make_card_props(3),
        ..Card::default()
    };

    let new_card = th
        .app
        .create_card(card.clone(), &board.id, &user_id, ChangeNotification::Send)
        .await
        .expect("the card is created");

    assert_eq!(new_card.board_id, card.board_id, "the board");
    assert_eq!(new_card.title, card.title, "the title");
    assert_eq!(new_card.properties, card.properties, "the properties");
    th.teardown().await;
}

/// Application-layer `create_card` fails when the block is rejected or the board does not exist.
#[tokio::test]
async fn app_create_card_error_scenario() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let user_id = th.stranger().await;

    let too_long = Card {
        title: "a".repeat(BLOCK_TITLE_MAX_CHARS + 1),
        ..Card::default()
    };
    let error = th
        .app
        .create_card(too_long, &board.id, &user_id, ChangeNotification::Send)
        .await
        .expect_err("the title is too long");
    assert_eq!(error.kind(), ErrorKind::BadRequest, "{error:?}");
    assert!(
        error
            .to_string()
            .ends_with(": block title size limit exceeded"),
        "what is wrong with the card, without the app layer's own words: {error}"
    );

    for board_id in [UNKNOWN_ID, "no-such-board"] {
        let error = th
            .app
            .create_card(
                Card::default(),
                board_id,
                &user_id,
                ChangeNotification::Send,
            )
            .await
            .expect_err("the board does not exist");
        assert!(error.is_not_found(), "{board_id}: {error:?}");
    }

    let cards = th
        .app
        .cards_for_board(
            &board.id,
            fvoci_server::app::ListPage {
                page: 0,
                per_page: fvoci_server::app::MAX_PER_PAGE,
            },
            fvoci_server::app::CardPreviews::Without,
        )
        .await
        .expect("the cards are read");
    assert_eq!(cards, [], "nothing was created");
    th.teardown().await;
}

/// Application-layer `get_cards` returns all cards on a board.
#[tokio::test]
async fn app_get_cards_success_scenario() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let mut blocks: Vec<Block> = (0..CARD_COUNT)
        .map(|number| Block {
            title: format!("card {number}"),
            ..new_block(&board.id, "", BlockType::Card)
        })
        .collect();
    th.app
        .insert_blocks(
            &mut blocks,
            &th.stranger().await,
            ChangeNotification::Suppress,
        )
        .await
        .expect("the card blocks are stored");

    let cards = th
        .app
        .cards_for_board(
            &board.id,
            fvoci_server::app::ListPage {
                page: 0,
                per_page: fvoci_server::app::MAX_PER_PAGE,
            },
            fvoci_server::app::CardPreviews::Without,
        )
        .await
        .expect("the cards are read");

    assert_eq!(cards.len(), CARD_COUNT, "all cards");
    th.teardown().await;
}

/// Malformed card blocks are rejected on write, and any previously stored block with a
/// malformed field is read safely without that field (see `card_from_stored_block` in
/// `app/cards.rs`) so listing cards does not fail.
#[tokio::test]
async fn app_get_cards_error_scenario() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = th
        .client
        .post(
            &format!("{TEST_WS}/containers-and-blocks"),
            &json!({
                "containers": [{"id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "open"}],
                "blocks": [{"id": "c1", "containerId": "b1", "type": "card", "fields": {"icon": 5}}],
            }),
        )
        .await;
    response.check_bad_request();
    assert_eq!(response.refused_at(), "/blocks/0/fields");

    let cards = th
        .app
        .cards_for_board(
            &board.id,
            fvoci_server::app::ListPage {
                page: 0,
                per_page: fvoci_server::app::MAX_PER_PAGE,
            },
            fvoci_server::app::CardPreviews::Without,
        )
        .await
        .expect("the cards are read");
    assert!(cards.is_empty(), "nothing was stored: {cards:?}");
    th.teardown().await;
}

/// Application-layer `patch_card` updates a card and returns the patched state.
#[tokio::test]
async fn app_patch_card_success_scenario() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let user_id = th.stranger().await;
    let props = make_card_props(3);
    let card = Card {
        title: "test card for patch".to_owned(),
        properties: props.clone(),
        ..Card::default()
    };
    let card = th
        .app
        .create_card(card, &board.id, &user_id, ChangeNotification::Send)
        .await
        .expect("the card is created");

    let card_patch = CardPatch {
        title: Some("patched".to_owned()),
        icon: Some("😀".to_owned()),
        updated_properties: props
            .keys()
            .map(|id| (id.clone(), json!(new_id())))
            .collect(),
        ..CardPatch::default()
    };
    let mut expected = card.clone();
    card_patch.apply(&mut expected);

    let patched = th
        .app
        .patch_card(
            &card_patch,
            &card.id,
            &user_id,
            ChangeNotification::Send,
            None,
        )
        .await
        .expect("the card is patched");

    assert_eq!(patched.board_id, board.id, "the board");
    assert_eq!(patched.title, "patched", "the title");
    assert_eq!(patched.icon, "😀", "the icon");
    assert_eq!(patched.properties, expected.properties, "the properties");
    th.teardown().await;
}

/// Application-layer `patch_card` fails for a missing card, an oversized title, or an invalid patch.
#[tokio::test]
async fn app_patch_card_error_scenario() {
    let th = Setup::native().start().await.init_basic().await;
    let (_, cards) = create_board_and_cards(&th, 1).await;
    let card = &cards[0];
    let patch_card = async |patch: &CardPatch, card_id: &str| {
        th.app
            .patch_card(
                patch,
                card_id,
                &th.stranger().await,
                ChangeNotification::Send,
                None,
            )
            .await
            .expect_err("the patch is refused")
    };

    for card_id in [UNKNOWN_ID, "no-such-card"] {
        let error = patch_card(&title_patch("x"), card_id).await;
        assert!(error.is_not_found(), "{card_id}: {error:?}");
        assert_eq!(
            error.to_string(),
            "not found",
            "nothing but that it is not there"
        );
    }

    let too_long = title_patch(&"a".repeat(BLOCK_TITLE_MAX_CHARS + 1));
    let error = patch_card(&too_long, &card.id).await;
    assert_eq!(error.kind(), ErrorKind::BadRequest, "{error:?}");

    let bad_icon = CardPatch {
        icon: Some("😀😀".to_owned()),
        ..CardPatch::default()
    };
    let error = patch_card(&bad_icon, &card.id).await;
    assert_eq!(error.kind(), ErrorKind::BadRequest, "{error:?}");

    let unchanged = th.app.card(&card.id).await.expect("the card is read");
    assert_eq!(&unchanged, card, "no patch was applied");
    th.teardown().await;
}

/// Application-layer `get_card` returns an existing card.
#[tokio::test]
async fn app_get_card_success_scenario() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let props = make_card_props(5);
    let content_order = vec![new_id(), new_id()];
    let fields = json!({
        "contentOrder": content_order,
        "properties": props,
        "icon": "😀",
        "isTemplate": true,
    });
    let mut block = Block {
        title: "test card".to_owned(),
        fields: serde_json::from_value(fields).expect("the fields are a JSON object"),
        ..new_block(&board.id, "", BlockType::Card)
    };
    let user_id = th.client.me().await.id;
    th.app
        .insert_block(&mut block, &user_id, ChangeNotification::Suppress)
        .await
        .expect("the card block is stored");

    let card = th.app.card(&block.id).await.expect("the card is read");

    assert_eq!(card.board_id, board.id, "the board");
    assert_eq!(card.title, block.title, "the title");
    assert_eq!(card.icon, "😀", "the icon");
    assert!(card.is_template, "a template");
    assert_eq!(card.properties, props, "the properties");
    th.teardown().await;
}

/// Application-layer `get_card` returns not found for an unknown card ID.
#[tokio::test]
async fn app_get_card_not_found() {
    let th = Setup::native().start().await.init_basic().await;

    for bogus_id in [UNKNOWN_ID, "nope"] {
        let error = th
            .app
            .card(bogus_id)
            .await
            .expect_err("there is no such block");

        assert!(error.is_not_found(), "{bogus_id}: {error:?}");
    }
    th.teardown().await;
}

/// Application-layer `get_card` fails when the target block is not a card.
#[tokio::test]
async fn app_get_card_error_scenario() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let view = insert_block(&th.client, &new_block(&board.id, "", BlockType::View)).await;

    let error = th.app.card(&view.id).await.expect_err("a view is no card");

    assert_eq!(error.kind(), ErrorKind::Internal, "{error:?}");
    assert_eq!(
        error.to_string(),
        "cannot convert block to card: not a card block",
        "what the handlers pass on"
    );
    th.teardown().await;
}

// -- POST /boards/{board_id}/cards --------------------------------------------------------

/// The server chooses the identifier, the board, the creator and the times of a new
/// card, whatever the request says, and stores the card as a block of the board.
#[tokio::test]
async fn create_card_answer_and_stored_block() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let me = th.client.me().await;
    let before = fvoci_server::auth::now_millis();

    // Everything a card can say about itself.
    let response = th
        .client
        .post(
            &cards_path(&board.id),
            &json!({
                "id": "myid",
                "boardId": board.id,
                "createdBy": "someone",
                "modifiedBy": "else",
                "title": "t",
                "contentOrder": ["a", "b"],
                "icon": "👨‍👩‍👧",
                "isTemplate": true,
                "properties": {"p": "v", "q": ["1", "2"]},
                "createdAt": 5,
                "updatedAt": 6,
                "deletedAt": 7,
            }),
        )
        .await;
    let card: Card = response.created_json();
    assert!(
        is_id(&card.id),
        "a new identifier, not the client's: {}",
        card.id
    );
    assert!(
        card.created_at >= before,
        "created now: {}",
        card.created_at
    );
    assert!(
        card.updated_at >= card.created_at,
        "written at {}, not before its creation at {}",
        card.updated_at,
        card.created_at
    );
    assert_eq!(
        response.value(),
        json!({
            "id": card.id,
            "boardId": board.id, "parentId": "",
            "number": null,
            "createdBy": me.id,
            "modifiedBy": me.id,
            "title": "t",
            "icon": "👨‍👩‍👧",
            "isTemplate": true,
            "properties": {"p": "v", "q": ["1", "2"]},
            "body": null,
            "hasBody": false,
            "tasksTotal": 0,
            "tasksDone": 0,
            "lastCommentAt": 0,
            "createdAt": card.created_at,
            "updatedAt": card.updated_at,
        }),
        "the answer is the card with the server's identifier, creator and times"
    );

    let block = th.app.block(&card.id).await.expect("the block is stored");
    assert_eq!(
        serde_json::to_value(&block).expect("a block is JSON"),
        json!({
            "id": card.id,
            "parentId": "",
            "createdBy": me.id,
            "modifiedBy": me.id,
            "type": "card",
            "title": "t",
            // A content order is no member of a card: what the request had is not stored.
            "fields": {
                "icon": "👨‍👩‍👧",
                "isTemplate": true,
                "properties": {"p": "v", "q": ["1", "2"]},
            },
            "body": null,
            "createdAt": card.created_at,
            "updatedAt": card.updated_at,
            "containerId": board.id,
        }),
        "a card block directly on its board, with the times that were answered"
    );

    th.teardown().await;
}

/// A card need not say anything about itself.
#[tokio::test]
async fn create_card_without_members() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let me = th.client.me().await;

    let response = th.client.post(&cards_path(&board.id), &json!({})).await;
    let card: Card = response.created_json();
    assert_eq!(
        response.value(),
        json!({
            "id": card.id,
            "boardId": board.id, "parentId": "",
            "number": 1,
            "createdBy": me.id,
            "modifiedBy": me.id,
            "title": "",
            "icon": "",
            "isTemplate": false,
            "properties": {},
            "body": null,
            "hasBody": false,
            "tasksTotal": 0,
            "tasksDone": 0,
            "lastCommentAt": 0,
            "createdAt": card.created_at,
            "updatedAt": card.updated_at,
        }),
        "an empty card"
    );

    th.teardown().await;
}

/// What `POST /boards/{id}/cards` refuses, in the order the checks are made: whether
/// the board can be seen, then the permission, then the body, then the card.
#[tokio::test]
async fn create_card_refusals() {
    let th = Setup::native().start().await.init_basic().await;
    let roles = board_with_roles(&th).await;
    let path = cards_path(&roles.board.id);
    let outsider = &th.client2;
    let denied = "access denied to create card";

    // Creating a card takes the permission to manage the board's cards. Who is no
    // member of the private board is told that there is no such board, as for a board
    // that does not exist; the body is not looked at.
    let card = json!({"title": "x"});
    for client in [&roles.commenter, &roles.viewer] {
        for body in [card.to_string().as_str(), "{\"title\":", "null"] {
            let response = client.send_raw_body(Method::POST, &path, body).await;
            check_error(&response, StatusCode::FORBIDDEN, denied);
        }
    }
    for body in [card.to_string().as_str(), "{\"title\":", "null"] {
        let response = outsider.send_raw_body(Method::POST, &path, body).await;
        check_error(&response, StatusCode::NOT_FOUND, "board not found");
    }
    for board_id in [UNKNOWN_ID, "no-such-board"] {
        let response = th.client.post(&cards_path(board_id), &card).await;
        check_error(&response, StatusCode::NOT_FOUND, "board not found");
    }

    // A body that is no card is a bad request.
    for body in [
        "{\"title\":",
        "",
        "[]",
        "null",
        "{\"title\":5}",
        "\"card\"",
        "{\"icon\":5}",
    ] {
        let response = th.client.send_raw_body(Method::POST, &path, body).await;
        response.check_bad_request();
        assert_eq!(response.problem_type(), "/problems/bad-request", "{body}");
    }

    let by_editor = create_card(&roles.editor, &roles.board.id, &card).await;
    assert_eq!(
        by_editor.created_by,
        roles.editor.me().await.id,
        "an editor creates cards"
    );

    let bad_icon = json!({"icon": "😱😱"});
    for other_board in [UNKNOWN_ID, "other"] {
        let response = th
            .client
            .post(&path, &json!({"boardId": other_board}))
            .await;
        check_error(
            &response,
            StatusCode::BAD_REQUEST,
            "the card names another board than the path",
        );
    }
    let response = th.client.post(&path, &bad_icon).await;
    check_error(
        &response,
        StatusCode::BAD_REQUEST,
        "an icon is at most one grapheme",
    );

    // The store's limits are the client's mistake too.
    let too_long = json!({"title": "a".repeat(BLOCK_TITLE_MAX_CHARS + 1)});
    let response = th.client.post(&path, &too_long).await;
    response.check_bad_request();
    let message = response.error_message();
    let block_id = message
        .strip_prefix("error validating block ")
        .and_then(|rest| rest.strip_suffix(": block title size limit exceeded"));
    assert!(
        block_id.is_some_and(is_id),
        "the message names the block by the identifier it would have had: {message}"
    );

    let cards = cards_of(&th.client, &roles.board.id, "").await;
    assert_eq!(cards, [by_editor], "only the editor's card was created");
    th.teardown().await;
}

// -- GET /boards/{board_id}/cards ---------------------------------------------------------

/// Everyone who may view the board gets its cards; a malformed paging parameter is
/// refused.
#[tokio::test]
async fn get_cards_permissions_and_malformed_paging() {
    let th = Setup::native().start().await.init_basic().await;
    let roles = board_with_roles(&th).await;
    let board_id = roles.board.id.as_str();
    let path = cards_path(board_id);
    let outsider = &th.client2;
    let card = create_card(&th.client, board_id, &json!({"title": "card"})).await;

    for client in [&th.client, &roles.editor, &roles.commenter, &roles.viewer] {
        assert_eq!(
            cards_of(client, board_id, "").await,
            std::slice::from_ref(&card),
            "every member reads the cards"
        );
    }
    // The private board answers an outsider as a board that does not exist.
    let response = outsider.get(&path).await;
    check_error(&response, StatusCode::NOT_FOUND, "board not found");
    for board_id in [UNKNOWN_ID, "no-such-board"] {
        let response = th.client.get(&cards_path(board_id)).await;
        check_error(&response, StatusCode::NOT_FOUND, "board not found");
    }

    // Invalid paging parameters return a single error response naming the parameter.
    let invalid = |name: &str, reason: &str| format!("invalid `{name}` parameter: {reason}");
    let cases = [
        ("?page=x", "page", "not a whole number"),
        ("?perPage=x", "perPage", "not a whole number"),
        ("?perPage=1.5", "perPage", "not a whole number"),
        ("?perPage=%202", "perPage", "not a whole number"),
        ("?page=99999999999999999999", "page", "out of range"),
    ];
    for (query, name, reason) in cases {
        let response = th.client.get(&format!("{path}{query}")).await;
        check_error(&response, StatusCode::BAD_REQUEST, &invalid(name, reason));
        assert_eq!(
            response.refused_at(),
            format!("parameter {name}"),
            "{query}"
        );
    }
    // Both wrong: both are told, each with its own words.
    let response = th.client.get(&format!("{path}?page=x&perPage=y")).await;
    check_error(
        &response,
        StatusCode::BAD_REQUEST,
        "2 things of the request were refused",
    );
    assert_eq!(
        response.refused_places(),
        ["parameter page", "parameter perPage"]
    );
    let details: Vec<String> = response.value()["errors"]
        .as_array()
        .expect("errors")
        .iter()
        .map(|entry| entry["detail"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        details,
        [
            invalid("page", "not a whole number"),
            invalid("perPage", "not a whole number")
        ]
    );
    // Whether the board can be seen comes first.
    let response = outsider.get(&format!("{path}?page=x")).await;
    check_error(&response, StatusCode::NOT_FOUND, "board not found");

    th.teardown().await;
}

/// `page` counts from zero and defaults to it, `perPage` defaults to 50 and is 1 to
/// 1000: there is no asking for all cards at once.
#[tokio::test]
async fn get_cards_paging() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let total = 103;
    let mut blocks: Vec<Block> = (0..total)
        .map(|number| Block {
            title: number.to_string(),
            ..new_block(&board.id, "", BlockType::Card)
        })
        .collect();
    th.app
        .insert_blocks(
            &mut blocks,
            &th.stranger().await,
            ChangeNotification::Suppress,
        )
        .await
        .expect("the card blocks are stored");
    let count = async |query: &str| cards_of(&th.client, &board.id, query).await.len();

    assert_eq!(count("").await, 50, "a page has 50 cards by default");
    assert_eq!(count("?page=2").await, 3, "the rest is on the third page");
    assert_eq!(count("?page=3").await, 0, "and nothing on the fourth");
    assert_eq!(count("?page=&perPage=").await, 50, "empty is absent");
    // The page size has one spelling; another is refused under strict query validation.
    for bad_query in ["?per_page=2", "?per_page=x"] {
        th.client
            .get(&format!("{}{bad_query}", cards_path(&board.id)))
            .await
            .check_bad_request();
    }
    assert_eq!(count("?perPage=2").await, 2, "a smaller page");
    assert_eq!(count("?perPage=%2B2").await, 2, "a sign is fine");
    assert_eq!(count("?page=00&perPage=03").await, 3, "so are zeros");
    assert_eq!(count("?page=51&perPage=2").await, 1, "the last card");
    assert_eq!(count("?page=1000&perPage=2").await, 0, "beyond the end");
    assert_eq!(count("?perPage=1000").await, total, "the largest page");
    let path = cards_path(&board.id);
    for query in [
        "?perPage=0",
        "?page=1&perPage=0",
        "?perPage=1001",
        "?perPage=2147483647",
        "?perPage=-1",
        "?page=-1",
        "?page=2147483648",
        "?perPage=1000000000000",
    ] {
        let response = th.client.get(&format!("{path}{query}")).await;
        response.check_bad_request();
    }

    // Pages do not overlap and leave nothing out.
    let mut titles = BTreeSet::new();
    for page in 0..6 {
        let cards = cards_of(&th.client, &board.id, &format!("?page={page}&perPage=20")).await;
        titles.extend(cards.into_iter().map(|card| card.title));
    }
    assert_eq!(titles.len(), total, "every card is on one of the pages");

    th.teardown().await;
}

/// A card block whose `icon` is not a string or whose `isTemplate` is not a boolean is
/// refused (400 naming `fields`) even through the block routes, so invalid card metadata
/// cannot poison the board's card list. `null` stands for an absent field, and stored
/// `contentOrder` is ignored regardless of shape.
#[tokio::test]
async fn a_malformed_card_block_is_refused() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    create_card(&th.client, &board.id, &json!({"title": "fine"})).await;
    // Side-by-side content layout stores nested arrays in `contentOrder`.
    insert_card_block(&th, &board.id, json!({"contentOrder": ["a", ["b", "c"]]})).await;
    insert_card_block(&th, &board.id, json!({"contentOrder": "a"})).await;
    let nulls = insert_card_block(&th, &board.id, json!({"icon": null, "isTemplate": null})).await;
    let cards = cards_of(&th.client, &board.id, "").await;
    assert_eq!(cards.len(), 4, "four cards");
    let read: Card = th.client.get(&card_path(&nulls.id)).await.ok_json();
    assert_eq!(read.icon, "", "null is no icon");
    assert!(!read.is_template, "null is not a template");

    for fields in [
        json!({"icon": 5}),
        json!({"icon": ["x"]}),
        json!({"isTemplate": "yes"}),
        json!({"isTemplate": 1}),
    ] {
        let response = th
            .client
            .post(
                &format!("{TEST_WS}/containers-and-blocks"),
                &json!({
                    "containers": [{"id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "open"}],
                    "blocks": [{"id": "c1", "containerId": "b1", "type": "card", "fields": fields}],
                }),
            )
            .await;
        response.check_bad_request();
        assert_eq!(response.refused_at(), "/blocks/0/fields", "{fields}");

        // Nor can a card get it by a patch.
        let response = th
            .client
            .patch(
                &format!("{TEST_WS}/boards/{}/blocks/{}", board.id, nulls.id),
                &json!({"updatedFields": fields}),
            )
            .await;
        response.check_bad_request();
        assert_eq!(
            response.refused_at(),
            "/updatedFields",
            "the member of the patch: {fields}"
        );
        let batch = json!({"blockIds": [nulls.id], "blockPatches": [{"updatedFields": fields}]});
        let response = th
            .client
            .patch(&format!("{TEST_WS}/boards/{}/blocks", board.id), &batch)
            .await;
        response.check_bad_request();
        assert_eq!(
            response.refused_at(),
            "/blockPatches/0/updatedFields",
            "{fields}"
        );
    }
    // Another block may hold such fields: only a card is read as one.
    let attachment = Block {
        fields: serde_json::from_value(json!({"icon": 5})).expect("an object"),
        ..new_block(&board.id, &nulls.id, BlockType::Attachment)
    };
    insert_block(&th.client, &attachment).await;
    assert_eq!(
        cards_of(&th.client, &board.id, "").await.len(),
        4,
        "the list works"
    );

    th.teardown().await;
}

/// A card an older server stored with a malformed field is read without it, and is
/// written and copied without it: a request is refused for what it writes, not for
/// what was there.
#[tokio::test]
async fn a_card_stored_with_a_malformed_field_is_written_and_copied_without_it() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let card = create_card(&th.client, &board.id, &json!({"title": "old"})).await;
    let stored = serde_json::from_value(json!({"icon": 5, "isTemplate": "yes", "properties": []}))
        .expect("an object");
    th.app
        .store()
        .set_block_fields_unchecked(&card.id, &stored)
        .await
        .expect("written as an older server would have");

    let read: Card = th.client.get(&card_path(&card.id)).await.ok_json();
    assert_eq!(read.icon, "", "read without it");
    let patched: Card = th
        .client
        .patch(&card_path(&card.id), &title_patch("new"))
        .await
        .ok_json();
    assert_eq!(patched.title, "new", "the patch writes the title");
    let response = th
        .client
        .patch(
            &format!("{TEST_WS}/boards/{}/blocks/{}", board.id, card.id),
            &json!({"title": "newer"}),
        )
        .await;
    response.check_no_content();

    th.app
        .store()
        .set_block_fields_unchecked(&card.id, &stored)
        .await
        .expect("written again");
    th.client
        .post_empty(&format!("{TEST_WS}/containers/{}/duplicate", board.id))
        .await
        .check_created();
    th.client
        .post_empty(&format!(
            "{TEST_WS}/boards/{}/blocks/{}/duplicate",
            board.id, card.id
        ))
        .await
        .check_created();
    th.teardown().await;
}

/// A title with U+0000 is refused (400 naming `title`) on both databases: PostgreSQL
/// cannot store it in text, which was a 500 there.
#[tokio::test]
async fn a_title_with_u0000_is_refused() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let response = th
        .client
        .post(&cards_path(&board.id), &json!({"title": "a\u{0}b"}))
        .await;
    response.check_bad_request();
    assert_eq!(response.refused_at(), "/title");

    let block = Block {
        title: "a\u{0}b".to_owned(),
        ..new_block(&board.id, "", BlockType::View)
    };
    let response = th
        .client
        .insert_blocks(&board.id, std::slice::from_ref(&block))
        .await;
    response.check_bad_request();
    assert_eq!(response.refused_at(), "/0/title");

    let card = create_card(&th.client, &board.id, &json!({"title": "fine"})).await;
    let response = th
        .client
        .patch(
            &format!("{TEST_WS}/boards/{}/blocks/{}", board.id, card.id),
            &json!({"title": "a\u{0}b"}),
        )
        .await;
    response.check_bad_request();
    assert_eq!(response.refused_at(), "/title");

    for (patch, member) in [
        (json!({"title": "a\u{0}"}), "title"),
        (json!({"description": "\u{0}"}), "description"),
        (json!({"icon": "\u{0}"}), "icon"),
        (
            json!({"updatedProperties": {"k": "a\u{0}"}}),
            "updatedProperties",
        ),
        (
            json!({"updatedProperties": {"a\u{0}": "v"}}),
            "updatedProperties",
        ),
        (
            json!({"updatedCardProperties": [{"id": "p", "name": "a\u{0}", "type": "text"}]}),
            "updatedCardProperties",
        ),
    ] {
        let response = th
            .client
            .patch(&format!("{TEST_WS}/containers/{}", board.id), &patch)
            .await;
        response.check_bad_request();
        assert_eq!(response.refused_at(), format!("/{member}"), "{patch}");
    }
    let response = th
        .client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"workspaceId": TEST_WORKSPACE_ID, "visibility": "open", "title": "x\u{0}"}),
        )
        .await;
    response.check_bad_request();
    assert_eq!(response.refused_at(), "/title");
    th.teardown().await;
}

// -- GET /cards/{card_id} -----------------------------------------------------------------

/// Reading a single card requires view permission on the board, matching the board's card
/// list (so viewers and commenters can read it). Because the path names a card without a
/// board, nonexistent cards and cards on unreadable boards return the same 404.
#[tokio::test]
async fn get_card_permissions_and_refusals() {
    let th = Setup::native().start().await.init_basic().await;
    let roles = board_with_roles(&th).await;
    let board_id = roles.board.id.as_str();
    let outsider = &th.client2;
    let card = create_card(&th.client, board_id, &json!({"title": "card"})).await;
    let path = card_path(&card.id);

    for client in [&th.client, &roles.editor, &roles.commenter, &roles.viewer] {
        let fetched: Card = client.get(&path).await.ok_json();
        assert_eq!(fetched, card, "every member of the board reads the card");
    }
    let response = outsider.get(&path).await;
    check_error(&response, StatusCode::NOT_FOUND, "card not found");
    for client in [&roles.commenter, &roles.viewer] {
        assert_eq!(
            cards_of(client, board_id, "").await,
            std::slice::from_ref(&card),
            "the same card as in the list"
        );
    }

    // No card: an identifier nobody has, a block that is no card, a board.
    let text = insert_block(
        &th.client,
        &new_block(board_id, &card.id, BlockType::Attachment),
    )
    .await;
    for card_id in [UNKNOWN_ID, "nope", text.id.as_str(), board_id] {
        for client in [&th.client, outsider] {
            let response = client.get(&card_path(card_id)).await;
            check_error(&response, StatusCode::NOT_FOUND, "card not found");
        }
    }

    // A card block without fields is an empty card.
    let bare = insert_card_block(&th, board_id, json!({})).await;
    let response = th.client.get(&card_path(&bare.id)).await;
    response.check_ok();
    assert_eq!(
        response.value(),
        json!({
            "id": bare.id,
            "boardId": board_id, "parentId": "",
            "number": 2,
            "createdBy": bare.created_by,
            "modifiedBy": bare.modified_by,
            "title": "",
            "icon": "",
            "isTemplate": false,
            "properties": {},
            "body": null,
            "hasBody": false,
            "tasksTotal": 0,
            "tasksDone": 0,
            "lastCommentAt": 0,
            "createdAt": response.value()["createdAt"],
            "updatedAt": response.value()["updatedAt"],
        }),
        "the card of a bare card block"
    );

    th.teardown().await;
}

// -- PATCH /cards/{card_id} ---------------------------------------------------------------

/// What `PATCH /cards/{id}` refuses, in the order the checks are made: whether there is
/// such a card for the user, then the permission on its board, then the body.
#[tokio::test]
async fn patch_card_refusals() {
    let th = Setup::native().start().await.init_basic().await;
    let roles = board_with_roles(&th).await;
    let board_id = roles.board.id.as_str();
    let outsider = &th.client2;
    let card = create_card(&th.client, board_id, &json!({"title": "card"})).await;
    let path = card_path(&card.id);
    let text = insert_block(
        &th.client,
        &new_block(board_id, &card.id, BlockType::Attachment),
    )
    .await;
    let malformed = "{\"title\":";

    // No card: not found for everyone, whatever the body. A card on a board the user
    // cannot read answers the same.
    for card_id in [UNKNOWN_ID, "nope", text.id.as_str()] {
        for client in [&th.client, outsider] {
            let response = client
                .send_raw_body(Method::PATCH, &card_path(card_id), malformed)
                .await;
            check_error(&response, StatusCode::NOT_FOUND, "card not found");
        }
    }
    for body in ["{\"title\":\"x\"}", malformed] {
        let response = outsider.send_raw_body(Method::PATCH, &path, body).await;
        check_error(&response, StatusCode::NOT_FOUND, "card not found");
    }

    // No permission to manage the board's cards: refused, whatever the body.
    for client in [&roles.commenter, &roles.viewer] {
        for body in ["{\"title\":\"x\"}", malformed, "{\"icon\":\"🐿🐿🐿\"}"] {
            let response = client.send_raw_body(Method::PATCH, &path, body).await;
            check_error(
                &response,
                StatusCode::FORBIDDEN,
                "access denied to patch card",
            );
        }
    }

    // Malformed bodies, `null`, and icons longer than one grapheme are bad requests.
    for body in [
        malformed,
        "",
        "[]",
        "null",
        "{\"title\":5}",
        "{\"icon\":5}",
        "{\"icon\":\"🐿🐿🐿\"}",
    ] {
        let response = th.client.send_raw_body(Method::PATCH, &path, body).await;
        response.check_bad_request();
        assert_eq!(response.problem_type(), "/problems/bad-request", "{body}");
    }

    let too_long = title_patch(&"a".repeat(BLOCK_TITLE_MAX_CHARS + 1));
    let response = th.client.patch(&path, &too_long).await;
    check_error(
        &response,
        StatusCode::BAD_REQUEST,
        &format!(
            "error validating block {0}: block title size limit exceeded",
            card.id
        ),
    );

    let unchanged: Card = th.client.get(&path).await.ok_json();
    assert_eq!(unchanged, card, "no patch was applied");
    th.teardown().await;
}

/// Two patches that set different properties of a card at about the same time both keep
/// their value; of two that set the same property, the later one wins.
#[tokio::test]
async fn patches_of_different_properties_at_the_same_time_both_stay() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let card = create_card(&th.client, &board.id, &json!({"title": "t"})).await;
    let path = card_path(&card.id);
    for round in 0..10 {
        let first = json!({"updatedProperties": {"a": format!("a{round}")}});
        let second = json!({"updatedProperties": {"b": format!("b{round}")}});
        let (one, two) = tokio::join!(
            th.client.patch(&path, &first),
            th.client.patch(&path, &second),
        );
        one.check_ok();
        two.check_ok();
        let read: Card = th.client.get(&path).await.ok_json();
        assert_eq!(
            serde_json::to_value(&read.properties).expect("properties are JSON"),
            json!({"a": format!("a{round}"), "b": format!("b{round}")}),
            "both values of round {round}"
        );
    }

    for value in ["first", "later"] {
        th.client
            .patch(&path, &json!({"updatedProperties": {"a": value}}))
            .await
            .check_ok();
    }
    let read: Card = th.client.get(&path).await.ok_json();
    assert_eq!(read.properties["a"], json!("later"), "the later one wins");
    th.teardown().await;
}

/// `updatedProperties` is merged into the values a card has; `deletedProperties` takes
/// values away, also the last one, and wins over `updatedProperties`.
#[tokio::test]
async fn patch_card_merges_and_deletes_values() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let card = create_card(
        &th.client,
        &board.id,
        &json!({"title": "t", "properties": {"p": "v", "q": ["1", "2"]}}),
    )
    .await;
    let path = card_path(&card.id);
    let send = async |body: Value| -> Value {
        let card: Card = th.client.patch(&path, &body).await.ok_json();
        serde_json::to_value(&card.properties).expect("properties are JSON")
    };

    let values = send(json!({"updatedProperties": {"z": "only", "p": "w"}})).await;
    assert_eq!(
        values,
        json!({"p": "w", "q": ["1", "2"], "z": "only"}),
        "the others stay"
    );
    let values = send(json!({"deletedProperties": ["p", "q"]})).await;
    assert_eq!(values, json!({"z": "only"}), "deleted");
    let values =
        send(json!({"updatedProperties": {"z": "again"}, "deletedProperties": ["z"]})).await;
    assert_eq!(
        values,
        json!({}),
        "set and deleted: deleted, the last value too"
    );
    th.teardown().await;
}

/// What a patch changes and what it leaves alone.
#[tokio::test]
async fn patch_card_changes() {
    let th = Setup::native().start().await.init_basic().await;
    let roles = board_with_roles(&th).await;
    let board_id = roles.board.id.as_str();
    let me = th.client.me().await;
    let editor = roles.editor.me().await;
    let card = create_card(
        &th.client,
        board_id,
        &json!({
            "title": "t",
            "contentOrder": ["a", "b"],
            "icon": "😱",
            "isTemplate": true,
            "properties": {"p": "v", "q": ["1", "2"]},
        }),
    )
    .await;
    let path = card_path(&card.id);
    let send = async |client: &Client, path: &str, body: Value| -> Card {
        client.patch(path, &body).await.ok_json()
    };

    // The title alone; the answer is the whole card.
    let patched = send(&th.client, &path, json!({"title": "patched"})).await;
    assert!(patched.updated_at >= card.updated_at, "modified now");
    let expected = Card {
        title: "patched".to_owned(),
        updated_at: patched.updated_at,
        ..card.clone()
    };
    assert_eq!(patched, expected, "only the title changed");

    // Patches that change nothing but the modification time.
    for body in [
        json!({}),
        json!({"updatedProperties": {}}),
        json!({"title": null, "contentOrder": null, "icon": null}),
        json!({"isTemplate": false, "deletedAt": 5, "boardId": "x"}),
        json!({"deletedProperties": ["none"]}),
    ] {
        let patched = send(&th.client, &path, body.clone()).await;
        let expected = Card {
            updated_at: patched.updated_at,
            ..expected.clone()
        };
        assert_eq!(patched, expected, "nothing changed by {body}");
    }

    // The values that are not named stay; the deleted ones go (merging is tested on its
    // own below).
    let patched = send(
        &th.client,
        &path,
        json!({"updatedProperties": {"z": "only"}, "deletedProperties": ["p", "q"]}),
    )
    .await;
    assert_eq!(
        serde_json::to_value(&patched.properties).expect("properties are JSON"),
        json!({"z": "only"}),
        "the values"
    );

    // The icon, by an editor, without notification. A content order is not a member
    // of a card any more and changes nothing.
    let patched = send(
        &roles.editor,
        &format!("{path}?disableNotify=true"),
        json!({"contentOrder": ["x", "y", "z"], "icon": "🐿"}),
    )
    .await;
    assert!(
        serde_json::to_value(&patched).expect("a card is JSON")["contentOrder"].is_null(),
        "no order"
    );
    assert_eq!(patched.icon, "🐿", "the new icon");
    assert_eq!(patched.modified_by, editor.id, "modified by the editor");
    assert_eq!(patched.created_by, me.id, "still created by its creator");

    // The icon can be emptied.
    let patched = send(&th.client, &path, json!({"icon": ""})).await;
    assert_eq!(patched.icon, "", "no icon");
    assert_eq!(patched.modified_by, me.id, "modified by the admin again");

    let block = th.app.block(&card.id).await.expect("the block is stored");
    assert_eq!(block.title, "patched", "the block's title");
    assert_eq!(
        serde_json::to_value(&block.fields).expect("fields are JSON"),
        json!({
            "icon": "",
            "isTemplate": true,
            "properties": {"z": "only"},
        }),
        "the block's fields"
    );

    // A card block stored with empty fields can still be patched cleanly.
    let bare = insert_card_block(&th, board_id, json!({})).await;
    let patched = send(
        &th.client,
        &card_path(&bare.id),
        json!({"title": "x", "updatedProperties": {"a": "b"}}),
    )
    .await;
    assert_eq!(patched.title, "x", "the title of the bare card");
    assert_eq!(
        serde_json::to_value(&patched.properties).expect("properties are JSON"),
        json!({"a": "b"}),
        "its properties"
    );

    th.teardown().await;
}

// -- who is told about a change -----------------------------------------------------------

/// Creating and patching a card is broadcast as the change of its block to the
/// board's members who listen to its workspace, with or without `disableNotify`.
#[tokio::test]
async fn card_changes_are_broadcast() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let mut member =
        WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &user1.id).await;
    let mut outsider =
        WorkspaceSocket::subscribe(&th, &th.client2, TEST_WORKSPACE_ID, &user2.id).await;
    let path = cards_path(&board.id);

    for (path, title) in [
        (path.clone(), "card"),
        (format!("{path}?disableNotify=true"), "quiet card"),
    ] {
        let card: Card = th
            .client
            .post(&path, &json!({"title": title}))
            .await
            .created_json();
        let message = member.expect("blockChanged", "/block/id", &card.id).await;
        assert_eq!(
            message["workspaceId"],
            json!(TEST_WORKSPACE_ID),
            "{message}"
        );
        assert_eq!(message["block"]["type"], json!("card"), "{message}");
        assert_eq!(message["block"]["title"], json!(title), "{message}");
        assert_eq!(
            message["block"]["containerId"],
            json!(board.id),
            "{message}"
        );

        for (query, title) in [("", "patched"), ("?disableNotify=true", "patched quietly")] {
            th.client
                .patch(
                    &format!("{}{query}", card_path(&card.id)),
                    &title_patch(title),
                )
                .await
                .check_ok();
            let message = member.expect("blockChanged", "/block/id", &card.id).await;
            assert_eq!(message["block"]["title"], json!(title), "{message}");
        }
    }

    assert!(
        th.app.finish_background(BACKGROUND).await,
        "background work finishes"
    );
    assert_eq!(member.drain().await, [json!(null); 0], "nothing else");
    assert_eq!(
        outsider.drain().await,
        [json!(null); 0],
        "a user who is no member of the board hears nothing"
    );
    th.teardown().await;
}

/// Where the webhook test's receiver listens. The server's configuration is changed by
/// a plain function, which finds the address here.
static WEBHOOK_URL: OnceLock<String> = OnceLock::new();

/// Starts an HTTP server that keeps the JSON bodies posted to it, and puts its address
/// into [`WEBHOOK_URL`].
async fn start_webhook_receiver() -> (Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    let posted: Arc<Mutex<Vec<Value>>> = Arc::default();
    let keep = {
        let posted = Arc::clone(&posted);
        move |axum::Json(block): axum::Json<Value>| async move {
            posted.lock().expect("lock poisoned").push(block);
        }
    };
    let hook = axum::Router::new().route("/hook", axum::routing::post(keep));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("a local port is free");
    let address = listener.local_addr().expect("the listener has an address");
    WEBHOOK_URL
        .set(format!("http://{address}/hook"))
        .expect("only one test sets the webhook address");
    let serving = tokio::spawn(async move {
        axum::serve(listener, hook)
            .await
            .expect("the webhook receiver serves");
    });
    (posted, serving)
}

/// A created card, a patched card and a card whose content was reordered are posted
/// to the webhook, as their blocks; a subscription is not.
#[tokio::test]
async fn card_changes_call_the_webhook() {
    let (posted, serving) = start_webhook_receiver().await;
    let th = Setup::native()
        .configure(|config| {
            config.block_change_webhooks = WEBHOOK_URL.get().into_iter().cloned().collect();
        })
        .start()
        .await
        .init_basic()
        .await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let me = th.client.me().await;
    // What the webhook got since the last call, as (identifier, type, title) triples.
    let take_posted = async || -> Vec<(String, String, String)> {
        assert!(
            th.app.finish_background(BACKGROUND).await,
            "the webhook calls finish"
        );
        std::mem::take(&mut *posted.lock().expect("lock poisoned"))
            .iter()
            .map(|block| {
                let text = |member: &str| block[member].as_str().unwrap_or_default().to_owned();
                (text("id"), text("type"), text("title"))
            })
            .collect()
    };
    let card_triple =
        |card: &Card, title: &str| (card.id.clone(), "card".to_owned(), title.to_owned());
    let nothing: [(String, String, String); 0] = [];

    let card = create_card(&th.client, &board.id, &json!({"title": "card"})).await;
    assert_eq!(
        take_posted().await,
        [card_triple(&card, "card")],
        "a new card"
    );

    th.client
        .patch(
            &format!("{}?disableNotify=true", card_path(&card.id)),
            &title_patch("patched"),
        )
        .await
        .check_ok();
    assert_eq!(
        take_posted().await,
        [card_triple(&card, "patched")],
        "a patched card"
    );

    th.client
        .post(
            &format!("{TEST_WS}/subscriptions"),
            &json!({
                "blockType": "card",
                "blockId": card.id,
                "userId": me.id,
            }),
        )
        .await
        .check_created();
    th.client
        .delete(&format!("{TEST_WS}/subscriptions/{}/{}", card.id, me.id))
        .await
        .check_no_content();
    assert_eq!(take_posted().await, nothing, "subscriptions are not posted");

    th.teardown().await;
    serving.abort();
}

/// Posts a comment on the card and returns it as stored (the server says when).
async fn comment_on(client: &Client, card: &Card) -> Block {
    let comment = Block {
        id: new_id(),
        container_id: card.board_id.clone(),
        parent_id: card.id.clone(),
        block_type: BlockType::Comment,
        title: "said".to_owned(),
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    let stored: Vec<Block> = client
        .insert_blocks(&card.board_id, std::slice::from_ref(&comment))
        .await
        .created_json();
    // The next comment is posted a millisecond later at least.
    tokio::time::sleep(Duration::from_millis(3)).await;
    stored.into_iter().next().expect("the comment")
}

/// Both the card list and `getCard` report when a card's last comment was posted (zero if
/// none); deleted comments do not count, and commenting leaves the card's `updatedAt` unchanged.
#[tokio::test]
async fn a_card_says_when_it_was_last_commented_on() {
    let th = Setup::native().start().await.init_basic().await;
    let (board, cards) = create_board_and_cards(&th, 2).await;
    let card = &cards[0];
    let posted_at = |cards: &[Card]| -> Vec<(String, i64)> {
        let mut seen: Vec<(String, i64)> = cards
            .iter()
            .map(|card| (card.id.clone(), card.last_comment_at))
            .collect();
        seen.sort();
        seen
    };
    let expected = |first: i64| {
        let mut all = vec![(cards[0].id.clone(), first), (cards[1].id.clone(), 0)];
        all.sort();
        all
    };

    assert_eq!(
        posted_at(&cards_of(&th.client, &board.id, "").await),
        expected(0)
    );
    let earlier = comment_on(&th.client, card).await;
    let later = comment_on(&th.client, card).await;
    assert!(
        later.created_at > earlier.created_at,
        "posted one after the other"
    );
    assert_eq!(
        posted_at(&cards_of(&th.client, &board.id, "").await),
        expected(later.created_at)
    );
    // What is not a comment of the card does not count: a block on the card after it,
    // and a comment on the comment.
    tokio::time::sleep(Duration::from_millis(3)).await;
    insert_block(
        &th.client,
        &new_block(&board.id, &card.id, BlockType::Attachment),
    )
    .await;
    let reply = Block {
        parent_id: later.id.clone(),
        ..new_block(&board.id, &later.id, BlockType::Comment)
    };
    let replied = th
        .client
        .insert_blocks(&board.id, std::slice::from_ref(&reply))
        .await;
    assert!(
        replied.status.is_success() || replied.status == StatusCode::BAD_REQUEST,
        "{}",
        replied.text()
    );
    let first_page: Vec<Card> = cards_of(&th.client, &board.id, "?page=0&perPage=1").await;
    assert_eq!(
        posted_at(&first_page),
        [(card.id.clone(), later.created_at)],
        "a page of the list says it too"
    );
    let read: Card = th.client.get(&card_path(&card.id)).await.ok_json();
    assert_eq!(
        read.last_comment_at, later.created_at,
        "getCard says it too"
    );
    assert_eq!(
        read.updated_at, card.updated_at,
        "commenting does not touch the card"
    );

    let block = |id: &str| format!("{TEST_WS}/boards/{}/blocks/{id}", board.id);
    th.client.delete(&block(&later.id)).await.check_no_content();
    let read: Card = th.client.get(&card_path(&card.id)).await.ok_json();
    assert_eq!(
        read.last_comment_at, earlier.created_at,
        "a deleted comment does not count"
    );
    let patched: Card = th
        .client
        .patch(&card_path(&card.id), &title_patch("renamed"))
        .await
        .ok_json();
    assert_eq!(
        patched.last_comment_at, earlier.created_at,
        "patchCard says it too"
    );
    // A comment changed is not a comment posted.
    th.client
        .patch(&block(&earlier.id), &json!({"title": "said again"}))
        .await
        .check_no_content();
    let read: Card = th.client.get(&card_path(&card.id)).await.ok_json();
    assert_eq!(
        read.last_comment_at, earlier.created_at,
        "the time it was posted"
    );
    th.client
        .delete(&block(&earlier.id))
        .await
        .check_no_content();
    assert_eq!(
        posted_at(&cards_of(&th.client, &board.id, "").await),
        expected(0)
    );
}
