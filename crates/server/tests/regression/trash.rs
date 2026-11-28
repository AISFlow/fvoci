//! Scenario: the trash. Deleting a card, a comment, a view or a board, bringing it
//! back, and what does not come back.

use serde_json::json;

use crate::common::UNKNOWN_ID;
use crate::fixtures::{add_member, new_board, new_card};
use crate::harness::{API, ROOT_API, Session};

pub(crate) async fn trash(s: &mut Session) {
    let [alice, bob, carol, dave] = s.users("trash", ["alice", "bob", "carol", "dave"]).await;
    let board = new_board(s, &alice, "Trash", "private", json!({})).await;
    add_member(s, &alice, &board, &bob, "editor").await;
    add_member(s, &alice, &board, &carol, "viewer").await;
    let alice_id = s.user_id(&alice).await;
    let board_path = format!("{ROOT_API}/containers/{board}");
    let blocks = format!("{ROOT_API}/boards/{board}");

    // A card with a subcard and a comment on it, and a view next to it.
    let card = new_card(s, &alice, &board, "Card", json!({}), "create card").await;
    let deep = new_card(
        s,
        &alice,
        &board,
        "Subcard",
        json!({"parentId": card}),
        "create subcard",
    )
    .await;
    let view = s
        .post("create a view", &format!("{blocks}/views"))
        .json(json!({"title": "View", "viewType": "board"}))
        .user(&alice)
        .await
        .json()["view"]["id"]
        .as_str()
        .expect("the view's identifier")
        .to_owned();
    let text = s
        .post(
            "comment on the card",
            &format!("{blocks}/cards/{card}/comments"),
        )
        .json(json!({"body": {"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "Comment"}]},
        ]}}))
        .user(&alice)
        .await
        .string("id");
    let board = Board {
        alice,
        bob,
        carol,
        dave,
        alice_id,
        path: board_path,
        blocks,
        card,
        deep,
        view,
        text,
    };
    a_block_and_what_is_below_it(s, &board).await;
    a_block_deleted_on_its_own(s, &board).await;
    a_board(s, &board).await;
}

/// Who works on the board of the scenario, where it is, and what is on it.
struct Board {
    /// Its administrator.
    alice: String,
    /// An editor.
    bob: String,
    /// A viewer.
    carol: String,
    /// Never a member.
    dave: String,
    alice_id: String,
    /// The route of the board.
    path: String,
    /// The route of the board under `/boards`.
    blocks: String,
    card: String,
    /// The subcard of the card.
    deep: String,
    view: String,
    /// The comment on the card.
    text: String,
}

/// A card goes with what is below it, and comes back with it.
async fn a_block_and_what_is_below_it(s: &mut Session, board: &Board) {
    let Board {
        alice,
        bob,
        carol,
        dave,
        alice_id,
        blocks,
        card,
        deep,
        view,
        ..
    } = board;
    let subscriptions = format!("{ROOT_API}/subscriptions");
    let of_alice = format!("{subscriptions}/{alice_id}");
    s.post("subscribe to the card", &subscriptions)
        .json(json!({
            "blockType": "card", "blockId": card,
            "userId": alice_id,
        }))
        .user(alice)
        .await;
    let delete = format!("{blocks}/cards/batch-delete");
    let restore = format!("{blocks}/cards/batch-restore");
    let cards = format!("{blocks}/cards");
    s.post("delete the card", &delete)
        .json(json!({"cardIds": [card]}))
        .user(alice)
        .await;
    s.get("cards: the card took everything below it", &cards)
        .user(alice)
        .await;
    s.get("subscriptions: ended with the card", &of_alice)
        .user(alice)
        .await;
    s.post("undelete: viewer", &restore)
        .json(json!({"cardIds": [card]}))
        .user(carol)
        .await;
    s.post("undelete: non-member", &restore)
        .json(json!({"cardIds": [card]}))
        .user(dave)
        .await;
    s.post("undelete: unknown card", &restore)
        .json(json!({"cardIds": [UNKNOWN_ID]}))
        .user(alice)
        .await;
    s.post(
        "undelete: a view that is not deleted",
        &format!("{blocks}/views/{view}/restore"),
    )
    .user(alice)
    .await;
    s.post("undelete: a child brings back the whole deletion", &restore)
        .json(json!({"cardIds": [deep]}))
        .user(bob)
        .await;
    s.get("cards: all back, with their parents", &cards)
        .user(alice)
        .await;
    s.post("undelete: again", &restore)
        .json(json!({"cardIds": [card]}))
        .user(alice)
        .await;
    s.get("subscriptions: back with the card", &of_alice)
        .user(alice)
        .await;
}

/// A comment deleted on its own, then the card it was on: the comment comes back only
/// after its card.
async fn a_block_deleted_on_its_own(s: &mut Session, board: &Board) {
    let Board {
        alice,
        blocks,
        card,
        text,
        ..
    } = board;
    let comments = format!("{blocks}/cards/{card}/comments");
    s.delete("delete the comment", &format!("{comments}/{text}"))
        .user(alice)
        .await;
    s.post(
        "delete the card again",
        &format!("{blocks}/cards/batch-delete"),
    )
    .json(json!({"cardIds": [card]}))
    .user(alice)
    .await;
    s.post(
        "undelete the comment: its card is in the trash",
        &format!("{comments}/{text}/restore"),
    )
    .user(alice)
    .await;
    s.post(
        "undelete the card",
        &format!("{blocks}/cards/batch-restore"),
    )
    .json(json!({"cardIds": [card]}))
    .user(alice)
    .await;
    s.get("comments on the card: still deleted", &comments)
        .user(alice)
        .await;
    s.post(
        "undelete the comment",
        &format!("{comments}/{text}/restore"),
    )
    .user(alice)
    .await;
    s.get("comments on the card: back", &comments)
        .user(alice)
        .await;
}

/// A board goes with its members, its cards and its views, and comes back with them.
async fn a_board(s: &mut Session, board: &Board) {
    let Board {
        alice,
        bob,
        carol,
        dave,
        path: board_path,
        blocks,
        card,
        view,
        ..
    } = board;
    s.delete(
        "delete the view on its own",
        &format!("{blocks}/views/{view}"),
    )
    .user(alice)
    .await;
    s.delete("delete the board", board_path).user(alice).await;
    s.get("board: gone", board_path).user(alice).await;
    s.get(
        "members: gone with the board",
        &format!("{board_path}/members"),
    )
    .user(alice)
    .await;
    s.get(
        "memberships of a member: none of the board",
        &format!("{API}/users/me/memberships"),
    )
    .user(bob)
    .await;
    s.get(
        "boards of the workspace: not listed",
        &format!("{ROOT_API}/boards"),
    )
    .user(alice)
    .await;
    s.post(
        "undelete a card of the deleted board",
        &format!("{blocks}/cards/batch-restore"),
    )
    .json(json!({"cardIds": [card]}))
    .user(alice)
    .await;
    s.post(
        "undelete the board: viewer",
        &format!("{board_path}/restore"),
    )
    .user(carol)
    .await;
    s.post(
        "undelete the board: editor",
        &format!("{board_path}/restore"),
    )
    .user(bob)
    .await;
    s.post(
        "undelete the board: never a member",
        &format!("{board_path}/restore"),
    )
    .user(dave)
    .await;
    s.post(
        "undelete: unknown board",
        &format!("{ROOT_API}/containers/{UNKNOWN_ID}/restore"),
    )
    .user(alice)
    .await;
    s.post("undelete the board", &format!("{board_path}/restore"))
        .user(alice)
        .await;
    s.get("board: back", board_path).user(alice).await;
    s.get(
        "members: back with their roles",
        &format!("{board_path}/members"),
    )
    .user(alice)
    .await;
    s.get("cards: back", &format!("{blocks}/cards"))
        .user(bob)
        .await;
    s.get(
        "views: back, but for the view deleted before",
        &format!("{blocks}/views"),
    )
    .user(bob)
    .await;
    s.post(
        "undelete the board: again",
        &format!("{board_path}/restore"),
    )
    .user(alice)
    .await;
}
