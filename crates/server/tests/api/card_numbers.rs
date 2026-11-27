//! The numbers of the cards of a board, as the routes give them, and a card found by
//! its number (`getCardByNumber`) only by whoever reads it.

use fvoci_model::{Block, BlockType, Card, ContainerKind, ContainerVisibility, new_id};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{
    ApiResponse, Client, HIDDEN_WORKSPACE_ID, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TEST_WS,
    TestServer, UNKNOWN_ID,
};
use crate::export::import;

/// Makes a card on the board as the client, by `createCard`, and returns it.
async fn create_card(client: &Client, at: &str, board_id: &str, card: Value) -> Card {
    client
        .post(&format!("{at}/boards/{board_id}/cards"), &card)
        .await
        .created_json()
}

/// Asks for the card of the board by its number, as written in the path.
async fn by_number(client: &Client, at: &str, board_id: &str, number: &str) -> ApiResponse {
    client
        .get(&format!("{at}/boards/{board_id}/cards/by-number/{number}"))
        .await
}

/// The numbers of the cards of a board as `getCards` lists them, by title and number.
async fn listed_numbers(client: &Client, at: &str, board_id: &str) -> Vec<(String, Value)> {
    let cards: Vec<Value> = client
        .get(&format!("{at}/boards/{board_id}/cards"))
        .await
        .ok_json();
    let mut numbers: Vec<(String, Value)> = cards
        .iter()
        .map(|card| {
            (
                card["title"].as_str().expect("a title").to_owned(),
                card["number"].clone(),
            )
        })
        .collect();
    numbers.sort_by_key(|(title, number)| (title.clone(), number.to_string()));
    numbers
}

#[tokio::test]
async fn every_way_a_card_is_made_gives_it_the_next_number_and_every_read_says_it() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let first = create_card(
        &ts.client,
        TEST_WS,
        &board.id,
        json!({"title": "first", "number": 40}),
    )
    .await;
    assert_eq!(first.number, Some(1), "not the number the request sent");
    let template = json!({"title": "template", "isTemplate": true});
    let template = create_card(&ts.client, TEST_WS, &board.id, template).await;
    assert_eq!(
        template.number, None,
        "a template of the board's cards has none"
    );

    let sent = Block {
        id: new_id(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        title: "by blocks".to_owned(),
        number: Some(1),
        ..Block::default()
    };
    ts.client
        .insert_blocks_in(TEST_WORKSPACE_ID, &board.id, &[sent])
        .await
        .check_bad_request();
    let second_req = json!({"title": "by blocks", "number": 1});
    let second = create_card(&ts.client, TEST_WS, &board.id, second_req).await;
    assert_eq!(second.number, Some(2));

    let duplicate = |id: String, query: &'static str| {
        let client = &ts.client;
        let board_id = board.id.clone();
        async move {
            let copies: Vec<Block> = client
                .post_empty(&format!(
                    "{TEST_WS}/boards/{board_id}/blocks/{id}/duplicate{query}"
                ))
                .await
                .created_json();
            copies[0].number
        }
    };
    assert_eq!(duplicate(first.id.clone(), "").await, Some(3), "a copy");
    assert_eq!(
        duplicate(template.id.clone(), "").await,
        Some(4),
        "a card made from a template"
    );
    assert_eq!(
        duplicate(first.id.clone(), "?asTemplate=true").await,
        None,
        "a template made from a card"
    );

    let patched: Card = ts
        .client
        .patch(
            &format!("{TEST_WS}/cards/{}", first.id),
            &json!({"title": "first, patched"}),
        )
        .await
        .ok_json();
    assert_eq!(patched.number, Some(1), "a change keeps it");
    let read: Card = ts
        .client
        .get(&format!("{TEST_WS}/cards/{}", first.id))
        .await
        .ok_json();
    assert_eq!(read.number, Some(1));
    // A copy has the title of what it was copied from.
    let listed = listed_numbers(&ts.client, TEST_WS, &board.id).await;
    let expected = [
        ("by blocks", json!(2)),
        ("first", json!(3)),
        ("first", json!(null)),
        ("first, patched", json!(1)),
        ("template", json!(4)),
        ("template", json!(null)),
    ]
    .map(|(title, number)| (title.to_owned(), number));
    assert_eq!(listed, expected);
    let found: Card = by_number(&ts.client, TEST_WS, &board.id, "2")
        .await
        .ok_json();
    assert_eq!(found.id, second.id);
    assert_eq!(found.number, Some(2));

    // What a reference shows of the card says its number too.
    let resolved: Vec<Value> = ts
        .client
        .post(&format!("{TEST_WS}/references/resolve"), &[&first.id])
        .await
        .ok_json();
    assert_eq!(resolved[0]["number"], json!(1));

    ts.teardown().await;
}

/// An import is a new board of new cards: numbered from 1 on it, whatever numbers the
/// archive carries; a template of its cards has none there either.
#[tokio::test]
async fn an_imported_board_numbers_its_cards_from_one() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let mut made = Vec::new();
    for title in ["gone", "second", "third"] {
        made.push(create_card(&ts.client, TEST_WS, &board.id, json!({"title": title})).await);
    }
    let template = json!({"title": "template", "isTemplate": true});
    create_card(&ts.client, TEST_WS, &board.id, template).await;
    // The archive has the numbers 2 and 3.
    ts.client
        .delete(&format!(
            "{TEST_WS}/boards/{}/blocks/{}",
            board.id, made[0].id
        ))
        .await
        .check_no_content();
    let exported = ts
        .client
        .get(&format!("{TEST_WS}/containers/{}/archive/export", board.id))
        .await;
    exported.check_ok();
    let before = root_boards(&ts).await;
    import(&ts.client, ROOT_WORKSPACE_ID, &exported.body)
        .await
        .check_ok();
    let imported: Vec<String> = root_boards(&ts)
        .await
        .into_iter()
        .filter(|board_id| !before.contains(board_id))
        .collect();
    let [imported] = imported.as_slice() else {
        panic!("one board is imported: {imported:?}");
    };
    let root = format!("/w/{ROOT_WORKSPACE_ID}");
    // In the order the board lists its cards, which is the archive's: the numbers rise
    // with it, and the template has none.
    let cards: Vec<Value> = ts
        .client
        .get(&format!("{root}/boards/{imported}/cards"))
        .await
        .ok_json();
    let listed: Vec<(&str, &Value)> = cards
        .iter()
        .map(|card| (card["title"].as_str().expect("a title"), &card["number"]))
        .collect();
    assert_eq!(
        listed,
        [
            ("second", &json!(1)),
            ("third", &json!(2)),
            ("template", &json!(null))
        ],
        "{listed:?}"
    );
    ts.teardown().await;
}

/// What a request sends as a number, which is no number of a card at all, is ignored
/// as any number it sends is: by the route of cards and by that of blocks.
#[tokio::test]
async fn a_number_a_request_sends_is_never_refused_nor_taken() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let sent = json!({"title": "sent", "number": "7"});
    let card = create_card(&ts.client, TEST_WS, &board.id, sent).await;
    assert_eq!(card.number, Some(1));
    let second = create_card(
        &ts.client,
        TEST_WS,
        &board.id,
        json!({"title": "negative", "number": -1}),
    )
    .await;
    assert_eq!(second.number, Some(2));
    let combined: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/containers-and-blocks"),
            &json!({
                "containers": [{"id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "open"}],
                "blocks": [{"id": "a", "containerId": "b1", "type": "card", "number": -1}],
            }),
        )
        .await
        .created_json();
    assert_eq!(combined["blocks"][0]["number"], json!(1));
    ts.teardown().await;
}

/// The boards of the root workspace that the first user reads.
async fn root_boards(ts: &TestServer) -> Vec<String> {
    ts.app
        .containers_for_user_and_workspace(
            &ts.client.me().await.id,
            ROOT_WORKSPACE_ID,
            fvoci_model::PublicContainers::Include,
        )
        .await
        .expect("the boards of the root workspace")
        .into_iter()
        .filter(|container| container.kind == ContainerKind::Board)
        .map(|container| container.id)
        .collect()
}

/// The answer of an asker for a number no card of the board has: what every asker who
/// may not read the card is told too.
async fn no_such_card(ts: &TestServer, board_id: &str) -> (StatusCode, Value) {
    let answer = by_number(&ts.client, TEST_WS, board_id, "999").await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND, "{}", answer.text());
    answer.problem()
}

/// A card in the trash, and what is no number as `Card.number` writes it, answer as a
/// number no card of the board has.
#[tokio::test]
async fn what_is_no_card_of_the_board_now_is_not_found() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    create_card(&ts.client, TEST_WS, &board.id, json!({"title": "1"})).await;
    let second = create_card(&ts.client, TEST_WS, &board.id, json!({"title": "2"})).await;
    let nothing = no_such_card(&ts, &board.id).await;
    by_number(&ts.client, TEST_WS, &board.id, "1")
        .await
        .check_ok();
    ts.client
        .delete(&format!(
            "{TEST_WS}/boards/{}/blocks/{}",
            board.id, second.id
        ))
        .await
        .check_no_content();
    for number in [
        "2",
        "0",
        "-1",
        "+1",
        "01",
        "1.0",
        "a",
        "99999999999",
        UNKNOWN_ID,
    ] {
        let answer = by_number(&ts.client, TEST_WS, &board.id, number).await;
        assert_eq!(answer.problem(), nothing, "{number}");
    }
    ts.teardown().await;
}

#[tokio::test]
async fn a_card_by_its_number_is_there_for_who_reads_its_board_and_for_nobody_else() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let first = create_card(&ts.client, TEST_WS, &board.id, json!({"title": "1"})).await;
    let nothing = no_such_card(&ts, &board.id).await;

    // Who may not read the board: no member of a private board, a guest who is not
    // invited to it; and a board of another workspace, a wiki, and no board at all.
    let stranger = &ts.client2;
    let answer = by_number(stranger, TEST_WS, &board.id, "1").await;
    assert_eq!(answer.problem(), nothing, "no member of the private board");
    let elsewhere_by = ts.stranger_in(HIDDEN_WORKSPACE_ID).await;
    let mut elsewhere = fvoci_model::Container {
        id: new_id(),
        workspace_id: HIDDEN_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Open,
        ..fvoci_model::Container::default()
    };
    ts.app
        .store()
        .insert_container_with_admin(&mut elsewhere, &elsewhere_by)
        .await
        .expect("a board of a workspace the users are not in");
    let mut theirs = Block {
        id: new_id(),
        container_id: elsewhere.id.clone(),
        block_type: BlockType::Card,
        title: "theirs".to_owned(),
        ..Block::default()
    };
    ts.app
        .store()
        .insert_block(&mut theirs, &elsewhere_by)
        .await
        .expect("their card");
    assert_eq!(theirs.number, Some(1));
    let answer = by_number(&ts.client, TEST_WS, &elsewhere.id, "1").await;
    assert_eq!(answer.problem(), nothing, "a board of another workspace");
    let hidden = format!("/w/{HIDDEN_WORKSPACE_ID}");
    let (there, not_there) = (
        by_number(&ts.client2, &hidden, &elsewhere.id, "1").await,
        by_number(&ts.client2, &hidden, &elsewhere.id, "2").await,
    );
    assert_eq!(
        there.problem(),
        not_there.problem(),
        "a workspace one is not in"
    );
    let wiki: Value = ts
        .client
        .post(&format!("{TEST_WS}/wikis"), &json!({"visibility": "open"}))
        .await
        .created_json();
    let wiki_id = wiki["id"].as_str().expect("an id");
    let answer = by_number(&ts.client, TEST_WS, wiki_id, "1").await;
    assert_eq!(answer.problem(), nothing, "a wiki");
    let answer = by_number(&ts.client, TEST_WS, UNKNOWN_ID, "1").await;
    assert_eq!(answer.problem(), nothing, "no board");

    // A viewer reads it; a guest of the workspace only where invited.
    let viewer = ts.add_user("viewer").await;
    let viewer_id = viewer.me().await.id;
    let member = fvoci_model::ContainerMember {
        container_id: board.id.clone(),
        user_id: viewer_id,
        role: fvoci_model::ContainerRole::Viewer,
        synthetic: false,
        inherited_role: None,
    };
    ts.client.add_member(&member).await.check_ok();
    let read: Card = by_number(&viewer, TEST_WS, &board.id, "1").await.ok_json();
    assert_eq!(read.id, first.id);
    let open = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    create_card(&ts.client, TEST_WS, &open.id, json!({"title": "open"})).await;
    let open_nothing = no_such_card(&ts, &open.id).await;
    crate::guests::guest_of_the_workspace(&ts).await;
    let answer = by_number(&ts.client2, TEST_WS, &open.id, "1").await;
    assert_eq!(
        answer.problem(),
        open_nothing,
        "a guest is not invited to an open board"
    );
    ts.teardown().await;
}

/// A board of another organization is not there under this one's workspace.
#[tokio::test]
async fn a_card_of_another_organization_is_no_card() {
    // SQLite holds one organization.
    if fvoci_store::testing::db_type_of_the_tests() != fvoci_store::DbType::Postgres {
        return;
    }
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let nothing = no_such_card(&ts, &board.id).await;
    let (theirs, _, their_board) = crate::workspaces::another_organization(&ts).await;
    let owner = theirs
        .get_members_for_container(&their_board)
        .await
        .expect("their members")[0]
        .user_id
        .clone();
    let mut card = Block {
        id: new_id(),
        container_id: their_board.clone(),
        block_type: BlockType::Card,
        title: "theirs".to_owned(),
        ..Block::default()
    };
    theirs
        .insert_block(&mut card, &owner)
        .await
        .expect("their card is stored");
    assert_eq!(card.number, Some(1));
    let answer = by_number(&ts.client, TEST_WS, &their_board, "1").await;
    assert_eq!(answer.problem(), nothing);
    ts.teardown().await;
}

/// Verifies that a card template hides its number across card, block, reference,
/// shared-board and `getCardByNumber` routes.
async fn assert_template_hides_number_everywhere(
    ts: &TestServer,
    board_id: &str,
    card_id: &str,
    token: &str,
) {
    let fetched_card: Card = ts
        .client
        .get(&format!("{TEST_WS}/cards/{card_id}"))
        .await
        .ok_json();
    assert!(fetched_card.is_template, "card is marked as template");
    assert_eq!(fetched_card.number, None, "getCard hides template number");

    let blocks: Vec<Block> = ts
        .client
        .get(&format!(
            "{TEST_WS}/boards/{board_id}/blocks?blockId={card_id}"
        ))
        .await
        .ok_json();
    assert_eq!(blocks[0].number, None, "getBlocks hides template number");

    let resolved: Vec<Value> = ts
        .client
        .post(&format!("{TEST_WS}/references/resolve"), &[card_id])
        .await
        .ok_json();
    assert!(
        resolved[0].get("number").is_none(),
        "resolveReferences hides template number: {}",
        resolved[0]
    );

    let shared_cards: Vec<Card> = ts
        .anonymous()
        .get(&format!("/shared/{board_id}/cards?readToken={token}"))
        .await
        .ok_json();
    let shared_first = shared_cards
        .iter()
        .find(|card| card.id == card_id)
        .expect("template is in shared cards");
    assert_eq!(
        shared_first.number, None,
        "shared cards hide template number"
    );

    let shared_blocks: Vec<Block> = ts
        .anonymous()
        .get(&format!(
            "/shared/{board_id}/blocks?blockId={card_id}&readToken={token}"
        ))
        .await
        .ok_json();
    assert_eq!(
        shared_blocks[0].number, None,
        "shared blocks hide template number"
    );

    let nothing = no_such_card(ts, board_id).await;
    assert_eq!(
        by_number(&ts.client, TEST_WS, board_id, "1")
            .await
            .problem(),
        nothing,
        "getCardByNumber excludes templates"
    );

    let archive = ts
        .client
        .get(&format!("{TEST_WS}/containers/{board_id}/archive/export"))
        .await;
    archive.check_ok();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive.body)).expect("a ZIP archive");
    let mut jsonl = String::new();
    std::io::Read::read_to_string(
        &mut zip
            .by_name(&format!("{board_id}/container.jsonl"))
            .expect("container.jsonl"),
        &mut jsonl,
    )
    .expect("archive text");
    let exported_template = jsonl
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|line| line["type"] == "block" && line["data"]["id"] == card_id)
        .expect("template block in archive");
    assert!(
        exported_template["data"].get("number").is_none(),
        "archive export hides template number: {exported_template}"
    );
}

/// Converting a numbered card to a template hides its number across card, block,
/// reference, shared-board and `getCardByNumber` routes while preserving it in storage;
/// converting it back restores the old number without consuming a new one.
#[tokio::test]
async fn converting_a_card_to_template_hides_and_restores_its_number_across_routes() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let first = create_card(&ts.client, TEST_WS, &board.id, json!({"title": "first"})).await;
    let fresh_template = create_card(
        &ts.client,
        TEST_WS,
        &board.id,
        json!({"title": "fresh template", "isTemplate": true}),
    )
    .await;
    assert_eq!(first.number, Some(1), "first card takes #1");
    assert_eq!(fresh_template.number, None, "fresh template has no number");

    let sharing: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/public-link", board.id),
            &json!({"enabled": true}),
        )
        .await
        .ok_json();
    let token = sharing["token"].as_str().expect("a share token");

    // Convert card #1 into a template via patchBlock (`fields.isTemplate = true`).
    ts.client
        .patch(
            &format!("{TEST_WS}/boards/{}/blocks/{}", board.id, first.id),
            &json!({"updatedFields": {"isTemplate": true}}),
        )
        .await
        .check_no_content();
    assert_template_hides_number_everywhere(&ts, &board.id, &first.id, token).await;

    // Trash & restore while a template: still hidden, then restored on conversion back.
    ts.client
        .delete(&format!(
            "{TEST_WS}/boards/{}/blocks/{}",
            board.id, first.id
        ))
        .await
        .check_no_content();
    let undeleted: Block = ts
        .client
        .post_empty(&format!(
            "{TEST_WS}/boards/{}/blocks/{}/restore",
            board.id, first.id
        ))
        .await
        .ok_json();
    assert_eq!(undeleted.number, None, "restoreBlock hides template number");

    // Duplicate while template: ordinary copy gets #2, template copy gets None.
    let copied: Vec<Block> = ts
        .client
        .post_empty(&format!(
            "{TEST_WS}/boards/{}/blocks/{}/duplicate",
            board.id, first.id
        ))
        .await
        .created_json();
    assert_eq!(copied[0].number, Some(2), "duplicate gets a new number");
    let dup_template: Vec<Block> = ts
        .client
        .post_empty(&format!(
            "{TEST_WS}/boards/{}/blocks/{}/duplicate?asTemplate=true",
            board.id, first.id
        ))
        .await
        .created_json();
    assert_eq!(dup_template[0].number, None, "template duplicate has none");

    // Convert back to an ordinary card: regains #1 without consuming #3.
    ts.client
        .patch(
            &format!("{TEST_WS}/boards/{}/blocks/{}", board.id, first.id),
            &json!({"updatedFields": {"isTemplate": false}}),
        )
        .await
        .check_no_content();
    let restored_card: Card = ts
        .client
        .get(&format!("{TEST_WS}/cards/{}", first.id))
        .await
        .ok_json();
    assert!(!restored_card.is_template, "restored card is ordinary");
    assert_eq!(restored_card.number, Some(1), "old number is restored");
    let found_again: Card = by_number(&ts.client, TEST_WS, &board.id, "1")
        .await
        .ok_json();
    assert_eq!(found_again.id, first.id, "getCardByNumber finds #1 again");

    // Fresh template converted to ordinary card consumes the next number (#3).
    ts.client
        .patch(
            &format!("{TEST_WS}/boards/{}/blocks/{}", board.id, fresh_template.id),
            &json!({"updatedFields": {"isTemplate": false}}),
        )
        .await
        .check_no_content();
    let fresh_as_card: Card = ts
        .client
        .get(&format!("{TEST_WS}/cards/{}", fresh_template.id))
        .await
        .ok_json();
    assert_eq!(fresh_as_card.number, Some(3), "fresh template takes #3");
    ts.teardown().await;
}
