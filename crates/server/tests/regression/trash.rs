//! Scenario: the trash. Deleting a block or a board, bringing it back, and what does
//! not come back.

use serde_json::json;

use crate::common::UNKNOWN_ID;
use crate::fixtures::{add_member, block, id_of, new_blocks, new_board, new_card};
use crate::harness::{API, ROOT_API, Session};

pub(crate) async fn trash(s: &mut Session) {
    let [alice, bob, carol, dave] = s.users("trash", ["alice", "bob", "carol", "dave"]).await;
    let board = new_board(s, &alice, "Trash", "private", json!({})).await;
    add_member(s, &alice, &board, &bob, "editor").await;
    add_member(s, &alice, &board, &carol, "viewer").await;
    let alice_id = s.user_id(&alice).await;
    let board_path = format!("{ROOT_API}/containers/{board}");
    let blocks = format!("{ROOT_API}/boards/{board}/blocks");

    // A card with a subcard, comment and attachment on it, and a view next to it.
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
    let created = new_blocks(
        s,
        &alice,
        &board,
        json!([block(
            "v1",
            &board,
            "view",
            "View",
            "",
            json!({"viewType": "board"})
        )]),
        "insert a view",
    )
    .await;
    let view = id_of(&created[0]);
    let created = new_blocks(
        s,
        &alice,
        &board,
        json!([
            block("m1", &board, "comment", "Comment", &card, json!({})),
            block("t1", &board, "attachment", "Text", &card, json!({})),
        ]),
        "insert a comment and a text on the card",
    )
    .await;
    let text = id_of(&created[1]);
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
    /// The route of its blocks.
    blocks: String,
    card: String,
    /// The subcard of the card.
    deep: String,
    view: String,
    /// The text on the card.
    text: String,
}

/// A block goes with what is below it, and comes back with it.
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
    s.delete("delete the card", &format!("{blocks}/{card}"))
        .user(alice)
        .await;
    s.get("blocks: the card took everything below it", blocks)
        .user(alice)
        .await;
    s.get("subscriptions: ended with the card", &of_alice)
        .user(alice)
        .await;
    s.post("undelete: viewer", &format!("{blocks}/{card}/restore"))
        .user(carol)
        .await;
    s.post("undelete: non-member", &format!("{blocks}/{card}/restore"))
        .user(dave)
        .await;
    s.post(
        "undelete: unknown block",
        &format!("{blocks}/{UNKNOWN_ID}/restore"),
    )
    .user(alice)
    .await;
    s.post(
        "undelete: a block that is not deleted",
        &format!("{blocks}/{view}/restore"),
    )
    .user(alice)
    .await;
    s.post(
        "undelete: a child brings back the whole deletion",
        &format!("{blocks}/{deep}/restore"),
    )
    .user(bob)
    .await;
    s.get("blocks: all back, with their parents", blocks)
        .user(alice)
        .await;
    s.post("undelete: again", &format!("{blocks}/{card}/restore"))
        .user(alice)
        .await;
    s.get("subscriptions: back with the card", &of_alice)
        .user(alice)
        .await;
}

/// A block deleted on its own, then what it was on: the block comes back only after
/// what it was on.
async fn a_block_deleted_on_its_own(s: &mut Session, board: &Board) {
    let Board {
        alice,
        blocks,
        card,
        text,
        ..
    } = board;
    s.delete("delete the text", &format!("{blocks}/{text}"))
        .user(alice)
        .await;
    s.delete("delete the card again", &format!("{blocks}/{card}"))
        .user(alice)
        .await;
    s.post(
        "undelete the text: its card is in the trash",
        &format!("{blocks}/{text}/restore"),
    )
    .user(alice)
    .await;
    s.post("undelete the card", &format!("{blocks}/{card}/restore"))
        .user(alice)
        .await;
    s.get(
        "blocks on the card: the text is still deleted",
        &format!("{blocks}?parentId={card}"),
    )
    .user(alice)
    .await;
    s.post("undelete the text", &format!("{blocks}/{text}/restore"))
        .user(alice)
        .await;
    s.get(
        "blocks on the card: the text is back",
        &format!("{blocks}?parentId={card}"),
    )
    .user(alice)
    .await;
}

/// A board goes with its members and its blocks, and comes back with them.
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
    s.delete("delete the view on its own", &format!("{blocks}/{view}"))
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
        "undelete a block of the deleted board",
        &format!("{blocks}/{card}/restore"),
    )
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
    s.get("blocks: back, but for the view deleted before", blocks)
        .user(bob)
        .await;
    s.post(
        "undelete the board: again",
        &format!("{board_path}/restore"),
    )
    .user(alice)
    .await;
}
