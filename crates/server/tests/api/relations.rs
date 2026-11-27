//! Card relations: a property of the `type` `relation` whose value names cards of the
//! same board. Every way of writing a card checks what a value names; a card in the
//! trash may be named, and a card of a board the writer cannot see is answered as one
//! that is not there.

use fvoci_model::{Block, Card, Container, ContainerVisibility, new_id};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::TEST_WS;
use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID, TestServer};

const RELATION: &str = "rel";

/// Where a card patch's property values are.
const AT: &str = "/updatedProperties";

/// What a value that names a card not on the board is refused with.
const REASON: &str = "relation_card_not_found";

/// A private board of the first user with a relation property and a text property, and
/// cards on it.
struct Related {
    board: Container,
    cards: Vec<Card>,
}

async fn related_board(client: &Client, cards: usize) -> Related {
    let board: Container = client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({
                "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Related",
                "cardProperties": [
                    {"id": RELATION, "name": "After", "type": "relation", "options": []},
                    {"id": "note", "name": "Note", "type": "text", "options": []},
                ],
            }),
        )
        .await
        .created_json();
    let mut made = Vec::with_capacity(cards);
    for index in 0..cards {
        made.push(
            client
                .post(
                    &format!("{TEST_WS}/boards/{}/cards", board.id),
                    &json!({"title": format!("Card {index}")}),
                )
                .await
                .created_json(),
        );
    }
    Related { board, cards: made }
}

async fn patch_relation(client: &Client, card_id: &str, value: Value) -> ApiResponse {
    client
        .patch(
            &format!("{TEST_WS}/cards/{card_id}"),
            &json!({"updatedProperties": {RELATION: value}}),
        )
        .await
}

async fn relation_of(client: &Client, card_id: &str) -> Value {
    let card: Card = client
        .get(&format!("{TEST_WS}/cards/{card_id}"))
        .await
        .ok_json();
    card.properties
        .get(RELATION)
        .cloned()
        .unwrap_or(Value::Null)
}

/// Checks a refusal of a relation value: 400, the reason, and where in the request.
#[track_caller]
fn check_refused(response: &ApiResponse, reason: &str, place: &str) {
    assert_eq!(
        response.status,
        StatusCode::BAD_REQUEST,
        "{reason}: {}",
        response.text()
    );
    assert_eq!(response.error_reason(), reason, "{}", response.text());
    assert_eq!(
        response.problem_type(),
        format!("/problems/{}", reason.replace('_', "-")),
        "the problem of the reason"
    );
    assert_eq!(response.refused_at(), place, "{reason}");
}

/// The answer without what differs from one request to another.
fn comparable(response: &ApiResponse) -> (StatusCode, Value) {
    let mut body = response.value();
    if let Value::Object(members) = &mut body {
        members.remove("requestId");
        members.remove("instance");
    }
    (response.status, body)
}

/// A card of a board the first user cannot see: the second user's private board.
async fn hidden_card(th: &TestServer) -> String {
    let board: Container = th
        .client2
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Hidden"}),
        )
        .await
        .created_json();
    let card: Card = th
        .client2
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "x"}),
        )
        .await
        .created_json();
    card.id
}

/// A view of the board: a block of it that is no card.
async fn view_of(client: &Client, board_id: &str) -> String {
    let created: Vec<Block> = client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/blocks"),
            &json!([{
                "id": "v", "containerId": board_id, "type": "view",
                "title": "Table", "createdAt": 1, "updatedAt": 1,
                "fields": {"viewType": "table"},
            }]),
        )
        .await
        .created_json();
    created[0].id.clone()
}

fn link(card: &str) -> Value {
    json!({"card": card, "kind": "fs", "lagDays": 0})
}

fn many_links(count: usize) -> Value {
    Value::Array((0..count).map(|_| link(&new_id())).collect())
}

#[tokio::test]
async fn a_relation_names_other_cards_of_the_board_and_the_rest_is_refused_by_name() {
    let th = Setup::native().start().await.init_basic().await;
    let Related { board, cards } = related_board(&th.client, 3).await;
    let (first, second, third) = (&cards[0].id, &cards[1].id, &cards[2].id);
    let hidden = hidden_card(&th).await;
    let view = view_of(&th.client, &board.id).await;

    let cases = [
        ("a text", json!(second), "relation_not_a_list"),
        ("legacy string list", json!([second]), "relation_not_a_list"),
        (
            "the card itself",
            json!([link(second), link(first)]),
            "relation_to_itself",
        ),
        (
            "a card twice with same kind",
            json!([link(second), link(third), link(second)]),
            "relation_repeated",
        ),
        (
            "a card twice with different kind",
            json!([
                {"card": second, "kind": "fs", "lagDays": 0},
                {"card": second, "kind": "ss", "lagDays": 2}
            ]),
            "relation_repeated",
        ),
        ("51 cards", many_links(51), "relation_too_many"),
        ("a view", json!([link(&view)]), "relation_not_a_card"),
        (
            "no card at all",
            json!([link(&new_id())]),
            "relation_card_not_found",
        ),
        (
            "not an identifier",
            json!([link("c1")]),
            "relation_card_not_found",
        ),
        (
            "a card of a hidden board",
            json!([link(&hidden)]),
            "relation_card_not_found",
        ),
    ];
    for (what, value, reason) in cases {
        let response = patch_relation(&th.client, first, value).await;
        check_refused(&response, reason, AT);
        assert!(
            response.error_message().contains(&format!("{RELATION:?}")),
            "{what}: the words name the property: {}",
            response.text()
        );
    }
    let limit = patch_relation(&th.client, first, many_links(51)).await;
    assert_eq!(limit.value()["limit"], json!(50), "the limit is told");
    assert_eq!(
        relation_of(&th.client, first).await,
        Value::Null,
        "nothing was stored"
    );

    // Defaults omitted in request are normalized to `kind: "fs"` and `lagDays: 0`.
    let stored: Card = patch_relation(
        &th.client,
        first,
        json!([
            {"card": third},
            {"card": second, "kind": "ss", "lagDays": -3},
        ]),
    )
    .await
    .ok_json();
    assert_eq!(
        stored.properties[RELATION],
        json!([
            {"card": third, "kind": "fs", "lagDays": 0},
            {"card": second, "kind": "ss", "lagDays": -3},
        ]),
        "normalized and in order"
    );
    patch_relation(&th.client, first, Value::Null)
        .await
        .check_ok();
    assert_eq!(
        relation_of(&th.client, first).await,
        Value::Null,
        "null clears it"
    );
    th.teardown().await;
}

#[tokio::test]
async fn relation_kinds_lag_bounds_and_malformed_links_are_checked_strictly() {
    let th = Setup::native().start().await.init_basic().await;
    let Related { cards, .. } = related_board(&th.client, 5).await;
    let (target, c1, c2, c3, c4) = (
        &cards[0].id,
        &cards[1].id,
        &cards[2].id,
        &cards[3].id,
        &cards[4].id,
    );

    // All four kinds and lag bounds (-3650..=3650) are stored normalized.
    let accepted = json!([
        {"card": c1, "kind": "fs", "lagDays": -3650},
        {"card": c2, "kind": "ss", "lagDays": 0},
        {"card": c3, "kind": "ff", "lagDays": 3650},
        {"card": c4, "kind": "sf"},
    ]);
    let stored: Card = patch_relation(&th.client, target, accepted).await.ok_json();
    assert_eq!(
        stored.properties[RELATION],
        json!([
            {"card": c1, "kind": "fs", "lagDays": -3650},
            {"card": c2, "kind": "ss", "lagDays": 0},
            {"card": c3, "kind": "ff", "lagDays": 3650},
            {"card": c4, "kind": "sf", "lagDays": 0},
        ])
    );

    // Malformed link objects and relation links on non-relation properties are 400 refused.
    for bad in [
        json!([{"card": c1, "kind": "invalid"}]),
        json!([{"card": c1, "kind": "FS"}]),
        json!([{"card": c1, "kind": null}]),
        json!([{"card": c1, "lagDays": -3651}]),
        json!([{"card": c1, "lagDays": 3651}]),
        json!([{"card": c1, "lagDays": 1.5}]),
        json!([{"card": c1, "lagDays": "0"}]),
        json!([{"card": c1, "lagDays": null}]),
        json!([{"card": c1, "extra": true}]),
        json!([{"card": ""}]),
        json!([{}]),
    ] {
        let response = patch_relation(&th.client, target, bad.clone()).await;
        assert_eq!(
            response.status,
            StatusCode::BAD_REQUEST,
            "bad link {bad} should be 400: {}",
            response.text()
        );
        assert_eq!(
            response.problem_type(),
            "/problems/bad-request",
            "bad link {bad}: {}",
            response.text()
        );
    }

    let wrong_prop = th
        .client
        .patch(
            &format!("{TEST_WS}/cards/{target}"),
            &json!({"updatedProperties": {"note": [link(c1)]}}),
        )
        .await;
    assert_eq!(
        wrong_prop.status,
        StatusCode::BAD_REQUEST,
        "relation link list on a text property is 400: {}",
        wrong_prop.text()
    );
    assert_eq!(wrong_prop.problem_type(), "/problems/bad-request");
    th.teardown().await;
}

#[tokio::test]
async fn a_card_of_a_hidden_board_is_answered_as_one_that_is_not_there() {
    let th = Setup::native().start().await.init_basic().await;
    let Related { board, cards } = related_board(&th.client, 1).await;
    let hidden = hidden_card(&th).await;
    // Of the same length, so that the words could not differ by it.
    let none = new_id();

    for path in [format!("{TEST_WS}/cards/{}", cards[0].id), String::new()] {
        let answer = |id: &str| {
            let value = json!([link(id)]);
            let client = &th.client;
            let path = path.clone();
            let board_id = board.id.clone();
            async move {
                if path.is_empty() {
                    client
                        .post(
                            &format!("{TEST_WS}/boards/{board_id}/cards"),
                            &json!({"title": "new", "properties": {RELATION: value}}),
                        )
                        .await
                } else {
                    client
                        .patch(&path, &json!({"updatedProperties": {RELATION: value}}))
                        .await
                }
            }
        };
        let of_hidden = answer(&hidden).await;
        let of_none = answer(&none).await;
        assert_eq!(
            of_hidden.status,
            StatusCode::BAD_REQUEST,
            "{}",
            of_hidden.text()
        );
        assert_eq!(
            comparable(&of_hidden),
            comparable(&of_none),
            "the answers are the same: {path}"
        );
    }
    th.teardown().await;
}

#[tokio::test]
async fn a_card_in_the_trash_may_be_named_and_one_gone_for_good_may_not() {
    let th = Setup::native().start().await.init_basic().await;
    let Related { board, cards } = related_board(&th.client, 3).await;
    let (first, deleted, purged) = (&cards[0].id, &cards[1].id, &cards[2].id);
    for card in [deleted, purged] {
        th.client
            .delete(&format!("{TEST_WS}/boards/{}/blocks/{card}", board.id))
            .await
            .check_no_content();
    }
    patch_relation(&th.client, first, json!([link(deleted)]))
        .await
        .check_ok();
    // Gone for good: the purge removes what was deleted before now.
    th.client
        .post_empty(&format!(
            "{TEST_WS}/boards/{}/blocks/{deleted}/restore",
            board.id
        ))
        .await
        .check_ok();
    th.app
        .store()
        .purge_trash(i64::MAX)
        .await
        .expect("the trash is purged");
    check_refused(
        &patch_relation(&th.client, first, json!([link(deleted), link(purged)])).await,
        "relation_card_not_found",
        "/updatedProperties",
    );
    assert_eq!(
        relation_of(&th.client, first).await,
        json!([link(deleted)]),
        "the card that came back is named still"
    );
    th.teardown().await;
}

#[tokio::test]
async fn every_way_of_writing_a_card_checks_its_relations() {
    let th = Setup::native().start().await.init_basic().await;
    let Related { board, cards } = related_board(&th.client, 1).await;
    let card = &cards[0].id;
    let hidden = hidden_card(&th).await;
    let refused = json!([link(&hidden)]);

    let created = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "new", "properties": {RELATION: refused}}),
        )
        .await;
    check_refused(&created, REASON, "/properties");

    let mut block = json!({
        "id": "c", "containerId": "b", "type": "card", "title": "c",
        "fields": {"properties": {RELATION: refused}},
    });

    let patched = th
        .client
        .patch(
            &format!("{TEST_WS}/boards/{}/blocks/{card}", board.id),
            &json!({"updatedFields": {"properties": {RELATION: refused}}}),
        )
        .await;
    check_refused(&patched, REASON, "/updatedFields");

    let batch = th
        .client
        .patch(
            &format!("{TEST_WS}/boards/{}/blocks", board.id),
            &json!({
                "blockIds": [card],
                "blockPatches": [{"updatedFields": {"properties": {RELATION: refused}}}],
            }),
        )
        .await;
    check_refused(&batch, REASON, "/blockPatches/0/updatedFields");

    let both = th
        .client
        .patch(
            &format!("{TEST_WS}/containers-and-blocks"),
            &json!({
                "containerIds": [board.id], "containerPatches": [{}],
                "blockIds": [card],
                "blockPatches": [{"updatedFields": {"properties": {RELATION: refused}}}],
            }),
        )
        .await;
    check_refused(&both, REASON, "/blockPatches/0/updatedFields");

    block["containerId"] = json!("b");
    block["parentId"] = json!("");
    let new_board = th
        .client
        .post(
            &format!("{TEST_WS}/containers-and-blocks"),
            &json!({
                "containers": [{
                    "id": "b", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "new",
                    "cardProperties": [{"id": RELATION, "type": "relation"}],
                }],
                "blocks": [block],
            }),
        )
        .await;
    check_refused(&new_board, REASON, "/blocks/0/fields");

    assert_eq!(
        relation_of(&th.client, card).await,
        Value::Null,
        "nothing was stored"
    );
    th.teardown().await;
}

#[tokio::test]
async fn new_cards_of_one_request_may_name_each_other() {
    let th = Setup::native().start().await.init_basic().await;
    let made: Value = th
        .client
        .post(
            &format!("{TEST_WS}/containers-and-blocks"),
            &json!({
                "containers": [{
                    "id": "nb", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "new",
                    "cardProperties": [{"id": RELATION, "type": "relation"}],
                }],
                "blocks": [
                    {"id": "x", "containerId": "nb", "type": "card", "title": "x",
                     "fields": {"properties": {RELATION: [{"card": "y", "kind": "sf", "lagDays": 4}]}}},
                    {"id": "y", "containerId": "nb", "type": "card", "title": "y",
                     "fields": {"properties": {RELATION: [{"card": "x"}]}}},
                ],
            }),
        )
        .await
        .created_json();
    let blocks = made["blocks"].as_array().expect("blocks");
    assert_eq!(
        blocks[0]["fields"]["properties"][RELATION],
        json!([{"card": blocks[1]["id"], "kind": "sf", "lagDays": 4}])
    );
    assert_eq!(
        blocks[1]["fields"]["properties"][RELATION],
        json!([{"card": blocks[0]["id"], "kind": "fs", "lagDays": 0}])
    );
    th.teardown().await;
}

#[tokio::test]
async fn a_value_stored_before_the_rules_does_not_stop_other_changes() {
    let th = Setup::native().start().await.init_basic().await;
    let Related { cards, .. } = related_board(&th.client, 2).await;
    let (card, other) = (&cards[0].id, &cards[1].id);
    let missing = new_id();
    let mut stale = vec![link(card), link(&missing), link(&missing)];
    stale.extend((0..50).map(|_| link(&new_id())));
    let fields = json!({"properties": {RELATION: stale, "note": "before"}});
    th.app
        .store()
        .set_block_fields_unchecked(card, fields.as_object().expect("an object"))
        .await
        .expect("written as an older server would have");

    let patched: Card = th
        .client
        .patch(
            &format!("{TEST_WS}/cards/{card}"),
            &json!({"title": "renamed", "updatedProperties": {"note": "after"}}),
        )
        .await
        .ok_json();
    assert_eq!(patched.properties[RELATION], json!(stale), "left as it was");
    assert_eq!(patched.properties["note"], json!("after"));

    // Written, it is checked; what it named before is not asked for again.
    check_refused(
        &patch_relation(
            &th.client,
            card,
            json!([link(&missing), link(&missing), link(other)]),
        )
        .await,
        "relation_repeated",
        "/updatedProperties",
    );
    // Over the limit it may lose cards, and not take in another.
    check_refused(
        &patch_relation(
            &th.client,
            card,
            json!([&stale[3..], &[link(other)]].concat()),
        )
        .await,
        "relation_too_many",
        "/updatedProperties",
    );
    patch_relation(&th.client, card, json!(stale[2..]))
        .await
        .check_ok();
    patch_relation(&th.client, card, json!([link(&missing), link(other)]))
        .await
        .check_ok();
    check_refused(
        &patch_relation(
            &th.client,
            card,
            json!([link(&missing), link(other), link(&new_id())]),
        )
        .await,
        "relation_card_not_found",
        "/updatedProperties",
    );
    th.teardown().await;
}

#[tokio::test]
async fn a_change_of_the_property_kind_leaves_the_values_alone() {
    let th = Setup::native().start().await.init_basic().await;
    let Related { board, cards } = related_board(&th.client, 2).await;
    let (card, other) = (&cards[0].id, &cards[1].id);
    patch_relation(&th.client, card, json!([link(other)]))
        .await
        .check_ok();
    let path = format!("{TEST_WS}/containers/{}", board.id);
    for kind in ["text", "relation"] {
        th.client
            .patch(
                &path,
                &json!({"updatedCardProperties": [{"id": RELATION, "name": "After", "type": kind}]}),
            )
            .await
            .check_ok();
        assert_eq!(
            relation_of(&th.client, card).await,
            json!([link(other)]),
            "as {kind}"
        );
    }
    th.teardown().await;
}

/// A relation names cards of its own card's board, also where the writer sees another.
#[tokio::test]
async fn the_board_of_a_relation_is_the_board_of_the_card() {
    let th = Setup::native().start().await.init_basic().await;
    let Related { cards, .. } = related_board(&th.client, 1).await;
    let other = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    th.client
        .patch(
            &format!("{TEST_WS}/containers/{}", other.id),
            &json!({"updatedCardProperties": [{"id": RELATION, "type": "relation"}]}),
        )
        .await
        .check_ok();
    let card: Card = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", other.id),
            &json!({"title": "x"}),
        )
        .await
        .created_json();
    // The user sees both boards; a card of one is no card of the other.
    check_refused(
        &patch_relation(&th.client, &card.id, json!([link(&cards[0].id)])).await,
        "relation_card_not_found",
        "/updatedProperties",
    );
    th.teardown().await;
}

/// A block that becomes a card has its relation values checked as written: nothing
/// checked them while it was no card.
#[tokio::test]
async fn a_block_that_becomes_a_card_has_its_relations_checked() {
    let th = Setup::native().start().await.init_basic().await;
    let client = &th.client;
    let Related { board, cards } = related_board(client, 1).await;
    let blocks = format!("{TEST_WS}/boards/{}/blocks", board.id);
    for (id, value) in [
        ("odd", json!({RELATION: [link(&new_id())]})),
        ("fine", json!({RELATION: [link(&cards[0].id)]})),
    ] {
        client
            .post(
                &blocks,
                &json!([{
                    "id": id, "containerId": board.id, "parentId": cards[0].id, "type": "attachment",
                    "title": id,
                    "fields": {"properties": value},
                }]),
            )
            .await
            .check_created();
    }
    let created: Vec<Block> = client.get(&blocks).await.ok_json();
    let id_of = |title: &str| {
        created
            .iter()
            .find(|block| block.title == title)
            .map(|block| block.id.clone())
            .expect("the attachment")
    };
    let to_card = json!({"type": "card"});
    let odd = client
        .patch(&format!("{blocks}/{}", id_of("odd")), &to_card)
        .await;
    odd.check_bad_request();
    assert_eq!(odd.refused_at(), "/type", "{}", odd.text());
    let fine = client
        .patch(&format!("{blocks}/{}", id_of("fine")), &to_card)
        .await;
    fine.check_bad_request();
    assert_eq!(fine.refused_at(), "/type", "{}", fine.text());
    let after: Vec<Block> = client.get(&blocks).await.ok_json();
    let type_of = |title: &str| {
        after
            .iter()
            .find(|block| block.title == title)
            .map(|block| block.block_type)
    };
    assert_eq!(
        type_of("odd"),
        Some(fvoci_model::BlockType::Attachment),
        "odd stays attachment"
    );
    assert_eq!(
        type_of("fine"),
        Some(fvoci_model::BlockType::Attachment),
        "fine stays attachment"
    );
    th.teardown().await;
}
