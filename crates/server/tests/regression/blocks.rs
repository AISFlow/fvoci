//! Scenario: cards.
#![expect(
    clippy::too_many_lines,
    reason = "a scenario is one request sequence; cutting it up would hide the order it pins"
)]

use serde_json::{Value, json};

use crate::fixtures::{add_member, new_board};
use crate::harness::ROOT_API;
use crate::harness::{API, Body, Session};

pub(crate) async fn cards(s: &mut Session) {
    let [alice, bob] = s.users("cards", ["alice", "bob"]).await;
    let board = new_board(
        s,
        &alice,
        "Cards",
        "private",
        json!({
            "cardProperties": [
                {"id": "p1", "name": "P1", "type": "text", "options": []},
                {"id": "p2", "name": "P2", "type": "text", "options": []},
            ]
        }),
    )
    .await;
    add_member(s, &alice, &board, &bob, "viewer").await;
    let path = format!("{ROOT_API}/boards/{board}/cards");

    let first = s
        .post("create", &path)
        .json(json!({
            "title": "Card one", "icon": "🃏", "contentOrder": ["a", "b"], "properties": {"p1": "v1"},
        }))
        .user(&alice)
        .await;
    s.post("create: quiet", &format!("{path}?disableNotify=true"))
        .json(json!({"title": "Card two"}))
        .user(&alice)
        .await;
    s.post("create: viewer", &path)
        .json(json!({"title": "x"}))
        .user(&bob)
        .await;
    s.post("create: malformed body", &path)
        .raw(b"{")
        .user(&alice)
        .await;
    s.post(
        "create: unknown board",
        &format!("{ROOT_API}/boards/unknown-board/cards"),
    )
    .json(json!({"title": "x"}))
    .user(&alice)
    .await;
    s.get("list", &path).user(&alice).await;
    s.get("list: viewer", &path).user(&bob).await;
    s.get("list: paged", &format!("{path}?page=0&perPage=1"))
        .user(&alice)
        .unrecorded()
        .await;
    // Pins that an invalid pagination parameter returns the error alone with no trailing body.
    let bad_page = s
        .get("list: bad page", &format!("{path}?page=x"))
        .user(&alice)
        .unrecorded()
        .await;
    let text = match &bad_page.body {
        Body::Json(value) => value.to_string(),
        Body::Text(text) => text.clone(),
        Body::Bytes(bytes) => String::from_utf8_lossy(bytes).into_owned(),
    };
    // The first JSON value of the body, and whether anything comes after it. As JSON,
    // so that what differs from request to request is normalized like in any other
    // answer.
    let mut values = serde_json::Deserializer::from_str(&text).into_iter::<Value>();
    let first_object = match values.next() {
        Some(Ok(value)) => value,
        _ => Value::String(text.clone()),
    };
    let what_follows = if text[values.byte_offset()..].trim().is_empty() {
        "alone"
    } else {
        "followed by more"
    };
    s.note(
        "list: bad page",
        &json!([bad_page.status, first_object, what_follows]),
    );
    s.get(
        "list: unknown board",
        &format!("{ROOT_API}/boards/unknown-board/cards"),
    )
    .user(&alice)
    .await;
    let card = first.string("id");
    let card_path = format!("{ROOT_API}/cards/{card}");
    s.get("get", &card_path).user(&alice).await;
    s.get("get: viewer", &card_path).user(&bob).await;
    s.get(
        "get: unknown card",
        &format!("{ROOT_API}/cards/unknown-card"),
    )
    .user(&alice)
    .await;
    s.patch("patch", &card_path)
        .json(json!({
            "title": "Card one, renamed", "icon": "🂡", "contentOrder": ["b", "a"],
            "updatedProperties": {"p2": "v2"},
        }))
        .user(&alice)
        .await;
    s.patch("patch: viewer", &card_path)
        .json(json!({"title": "hijack"}))
        .user(&bob)
        .await;
    s.patch(
        "patch: unknown card",
        &format!("{ROOT_API}/cards/unknown-card"),
    )
    .json(json!({"title": "x"}))
    .user(&alice)
    .await;
    s.patch("patch: malformed body", &card_path)
        .raw(b"{")
        .user(&alice)
        .await;
    s.get("get after patch", &card_path).user(&alice).await;

    // A content order is no member of a card any more: setting one changes nothing, and
    // the route that moved content blocks is gone.
    s.patch("set content order", &card_path)
        .json(json!({"contentOrder": ["t1", "t2", "t3"]}))
        .user(&alice)
        .await;
    // The route that moved content blocks is gone: a card's content is its body.
    s.post(
        "move: the route is gone",
        &format!("{API}/content-blocks/t1/moveto/after/t3"),
    )
    .user(&alice)
    .await;
}
