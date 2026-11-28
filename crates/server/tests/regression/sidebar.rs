//! Scenarios: sidebar categories, sharing, subscriptions, search and the routes a
//! standalone server refuses.
#![expect(
    clippy::too_many_lines,
    reason = "a scenario is one request sequence; cutting it up would hide the order it pins"
)]

use serde_json::json;

use crate::common::ROOT_WORKSPACE_ID;
use crate::fixtures::{add_member, id_of, merged, new_board, new_card};
use crate::harness::ROOT_API;
use crate::harness::{API, Session};

pub(crate) async fn categories(s: &mut Session) {
    let [alice, bob] = s.users("cat", ["alice", "bob"]).await;
    let board = new_board(s, &alice, "Filed board", "private", json!({})).await;
    let other = new_board(s, &alice, "Other board", "private", json!({})).await;
    let alice_id = s.user_id(&alice).await;
    let bob_id = s.user_id(&bob).await;
    let path = format!("{ROOT_API}/categories");

    s.get("list: default only", &path).user(&alice).await;
    s.get("list: other user", &path).user(&bob).await;
    let created = s
        .post("create", &path)
        .json(json!({"name": "Projects", "userId": alice_id, "workspaceId": ROOT_WORKSPACE_ID}))
        .user(&alice)
        .await
        .json()
        .clone();
    let second = s
        .post("create second", &path)
        .json(json!({"name": "Archive", "userId": alice_id, "workspaceId": ROOT_WORKSPACE_ID, "collapsed": true}))
        .user(&alice)
        .await
        .json()
        .clone();
    s.post("create: no name", &path)
        .json(json!({"name": "", "userId": alice_id, "workspaceId": ROOT_WORKSPACE_ID}))
        .user(&alice)
        .await;
    s.post("create: for another user", &path)
        .json(json!({"name": "x", "userId": bob_id, "workspaceId": ROOT_WORKSPACE_ID}))
        .user(&alice)
        .await;
    s.post("create: for another workspace", &path)
        .json(json!({"name": "x", "userId": alice_id, "workspaceId": "1"}))
        .user(&alice)
        .await;
    s.post("create: bad type", &path)
        .json(json!({"name": "x", "userId": alice_id, "workspaceId": ROOT_WORKSPACE_ID, "type": "weird"}))
        .user(&alice)
        .await;
    s.post("create: malformed body", &path)
        .raw(b"{")
        .user(&alice)
        .await;
    let category = id_of(&created);
    let archive = id_of(&second);
    let listing = s.get("list", &path).user(&alice).await;
    let default = listing
        .json()
        .as_array()
        .expect("the categories are a list")
        .iter()
        .find(|category| category["name"] == "Boards")
        .map(id_of)
        .expect("the default category exists");

    s.put("update", &format!("{path}/{category}"))
        .json(merged(
            &created,
            json!({"name": "Projects 2", "collapsed": true}),
        ))
        .user(&alice)
        .await;
    s.put("update: id mismatch", &format!("{path}/{category}"))
        .json(merged(&created, json!({"id": archive})))
        .user(&alice)
        .await;
    s.put("update: other user", &format!("{path}/{category}"))
        .json(merged(&created, json!({"name": "hijack"})))
        .user(&bob)
        .await;
    s.put(
        "update: unknown category",
        &format!("{path}/unknown-category"),
    )
    .json(merged(&created, json!({"id": "unknown-category"})))
    .user(&alice)
    .await;
    s.put("update: system category", &format!("{path}/{default}"))
        .json(json!({
            "id": default, "name": "Renamed", "userId": alice_id, "workspaceId": ROOT_WORKSPACE_ID, "type": "system",
        }))
        .user(&alice)
        .await;

    let place = format!("{path}/{category}/containers/{board}");
    s.post("file a board", &place).user(&alice).await;
    s.post(
        "file the second board",
        &format!("{path}/{category}/containers/{other}"),
    )
    .user(&alice)
    .await;
    s.get("list after filing", &path).user(&alice).await;
    s.put("hide", &format!("{place}/hide")).user(&alice).await;
    s.put("hide: other user", &format!("{place}/hide"))
        .user(&bob)
        .await;
    s.get("list after hiding", &path).user(&alice).await;
    s.put("unhide", &format!("{place}/unhide"))
        .user(&alice)
        .await;

    let reorder_boards = format!("{path}/{category}/container-order");
    s.put("reorder boards", &reorder_boards)
        .json(json!([other, board]))
        .user(&alice)
        .await;
    s.put("reorder boards: missing one", &reorder_boards)
        .json(json!([other]))
        .user(&alice)
        .await;
    s.put("reorder boards: unknown board", &reorder_boards)
        .json(json!([other, "unknown"]))
        .user(&alice)
        .await;
    s.put("reorder boards: other user", &reorder_boards)
        .json(json!([other, board]))
        .user(&bob)
        .await;
    // The paths that ended in `reorder` are gone. The one for categories now reads as a
    // change of the category `reorder`, which is none.
    s.put(
        "old path: reorder boards",
        &format!("{path}/{category}/boards/reorder"),
    )
    .json(json!([other, board]))
    .user(&alice)
    .await;
    s.put("old path: reorder categories", &format!("{path}/reorder"))
        .json(json!([archive, category, default]))
        .user(&alice)
        .await;
    let reorder = format!("{ROOT_API}/category-order");
    s.put("reorder categories", &reorder)
        .json(json!([archive, category, default]))
        .user(&alice)
        .await;
    s.put("reorder categories: missing one", &reorder)
        .json(json!([archive, category]))
        .user(&alice)
        .await;
    s.put("reorder categories: unknown id", &reorder)
        .json(json!([archive, category, "unknown"]))
        .user(&alice)
        .await;
    s.get("list after reorder", &path).user(&alice).await;

    s.delete("delete: other user", &format!("{path}/{category}"))
        .user(&bob)
        .await;
    s.delete("delete: system category", &format!("{path}/{default}"))
        .user(&alice)
        .await;
    s.delete(
        "delete: unknown category",
        &format!("{path}/unknown-category"),
    )
    .user(&alice)
    .await;
    s.delete("delete", &format!("{path}/{category}"))
        .user(&alice)
        .await;
    s.delete("delete: again", &format!("{path}/{category}"))
        .user(&alice)
        .await;
    s.put("update: deleted category", &format!("{path}/{category}"))
        .json(merged(&created, json!({"name": "back"})))
        .user(&alice)
        .await;
    s.get("list at the end", &path).user(&alice).await;

    // Refused cross-user mutations: pinned last to confirm they leave state unchanged.
    s.post(
        "file a board: other user's category",
        &format!("{path}/{archive}/containers/{other}"),
    )
    .user(&bob)
    .await;
    s.get("list after the attempt on another user's category", &path)
        .user(&alice)
        .await;
    s.post(
        "file a board: unknown category",
        &format!("{path}/unknown-category/containers/{other}"),
    )
    .user(&alice)
    .await;
    s.get("list after filing into an unknown category", &path)
        .user(&alice)
        .await;
}

pub(crate) async fn public_links(s: &mut Session) {
    let [alice, bob] = s.users("share", ["alice", "bob"]).await;
    let board = new_board(s, &alice, "Shared board", "private", json!({})).await;
    add_member(s, &alice, &board, &bob, "editor").await;
    let card = new_card(
        s,
        &alice,
        &board,
        "Visible card",
        json!({}),
        "insert blocks",
    )
    .await;
    let path = format!("{ROOT_API}/containers/{board}/public-link");
    let board_path = format!("{ROOT_API}/containers/{board}");
    let shared_path = format!("{API}/shared/{board}");

    s.get("get: none yet", &path).user(&alice).await;
    s.post("post: editor", &path)
        .json(json!({"enabled": true}))
        .user(&bob)
        .await;
    s.post("post: with a token of the request's", &path)
        .json(json!({"enabled": true, "token": "share-token-1"}))
        .user(&alice)
        .await;
    s.post("post: nothing said", &path)
        .json(json!({}))
        .user(&alice)
        .await;
    s.get("get: still none", &path).user(&alice).await;
    let token = s
        .post("post", &path)
        .json(json!({"enabled": true}))
        .user(&alice)
        .await
        .string("token");
    s.post("post: malformed body", &path)
        .raw(b"{")
        .user(&alice)
        .await;
    s.get("get", &path).user(&alice).await;
    s.get("get: editor", &path).user(&bob).await;
    s.get(
        "get: unknown board",
        &format!("{ROOT_API}/containers/unknown-board/public-link"),
    )
    .user(&alice)
    .await;
    let again = s
        .post("post: again", &path)
        .json(json!({"enabled": true}))
        .user(&alice)
        .await
        .string("token");
    s.note("sharing again keeps the token", &json!(again == token));

    s.get(
        "board with the read token",
        &format!("{shared_path}?readToken={token}"),
    )
    .await;
    s.get(
        "board with a wrong read token",
        &format!("{shared_path}?readToken=wrong"),
    )
    .await;
    s.get(
        "board with the token under the old name of the parameter",
        &format!("{shared_path}?read_token={token}"),
    )
    .await;
    s.get(
        "blocks with the read token",
        &format!("{shared_path}/blocks?readToken={token}"),
    )
    .await;
    s.get(
        "blocks with a wrong read token",
        &format!("{shared_path}/blocks?readToken=wrong"),
    )
    .await;
    s.get(
        "members with the read token",
        &format!("{board_path}/members?readToken={token}"),
    )
    .await;
    s.post(
        "insert with the read token",
        &format!("{ROOT_API}/boards/{board}/cards?readToken={token}"),
    )
    .json(json!({"title": "x"}))
    .await;
    s.patch(
        "patch with the read token",
        &format!("{ROOT_API}/cards/{card}?readToken={token}"),
    )
    .json(json!({"title": "hijack"}))
    .await;

    let renewed = s
        .post("regenerate", &path)
        .json(json!({"enabled": true, "regenerate": true}))
        .user(&alice)
        .await
        .string("token");
    s.note(
        "a regenerated token is another one",
        &json!(renewed != token),
    );
    s.get(
        "board with the token from before regenerating",
        &format!("{shared_path}?readToken={token}"),
    )
    .await;
    s.get(
        "board with the regenerated token",
        &format!("{shared_path}?readToken={renewed}"),
    )
    .await;

    s.post("disable", &path)
        .json(json!({"enabled": false}))
        .user(&alice)
        .await;
    s.get("get: disabled", &path).user(&alice).await;
    s.get(
        "board with the token of a disabled share",
        &format!("{shared_path}?readToken={renewed}"),
    )
    .await;
    let last = s
        .post("enable again", &path)
        .json(json!({"enabled": true}))
        .user(&alice)
        .await
        .string("token");
    s.note(
        "sharing again after disabling makes a new token",
        &json!(last != renewed && last != token),
    );
    s.get(
        "board with the token from before disabling",
        &format!("{shared_path}?readToken={renewed}"),
    )
    .await;
}

pub(crate) async fn subscriptions(s: &mut Session) {
    let [alice, bob] = s.users("subs", ["alice", "bob"]).await;
    let board = new_board(s, &alice, "Subscribed board", "private", json!({})).await;
    let card = new_card(
        s,
        &alice,
        &board,
        "Watched card",
        json!({}),
        "insert blocks",
    )
    .await;
    let alice_id = s.user_id(&alice).await;
    let bob_id = s.user_id(&bob).await;
    let path = format!("{ROOT_API}/subscriptions");
    let subscription = |block_id: &str, user: &str| {
        json!({
            "blockType": "card", "blockId": block_id, "userId": user,
        })
    };

    s.get(
        "list: the card its writer wrote",
        &format!("{path}/{alice_id}"),
    )
    .user(&alice)
    .await;
    s.post("create", &path)
        .json(subscription(&card, &alice_id))
        .user(&alice)
        .await;
    s.post("create: again", &path)
        .json(subscription(&card, &alice_id))
        .user(&alice)
        .await;
    s.post("create: for another user", &path)
        .json(subscription(&card, &bob_id))
        .user(&alice)
        .await;
    s.post("create: no block", &path)
        .json(subscription("", &alice_id))
        .user(&alice)
        .await;
    s.post("create: malformed body", &path)
        .raw(b"{")
        .user(&alice)
        .await;
    s.get("list", &format!("{path}/{alice_id}"))
        .user(&alice)
        .await;
    s.get("list: another user's", &format!("{path}/{alice_id}"))
        .user(&bob)
        .await;
    s.delete(
        "delete: another user's",
        &format!("{path}/{card}/{alice_id}"),
    )
    .user(&bob)
    .await;
    s.delete("delete", &format!("{path}/{card}/{alice_id}"))
        .user(&alice)
        .await;
    s.delete("delete: again", &format!("{path}/{card}/{alice_id}"))
        .user(&alice)
        .await;
    s.get("list at the end", &format!("{path}/{alice_id}"))
        .user(&alice)
        .await;
}

pub(crate) async fn search(s: &mut Session) {
    let [alice, bob] = s.users("search", ["alice", "bob"]).await;
    new_board(s, &alice, "Quarterly zebra plan", "private", json!({})).await;
    new_board(s, &alice, "Open zebra notes", "open", json!({})).await;
    new_board(s, &bob, "Private zebra diary", "private", json!({})).await;
    let workspace_search = format!("{ROOT_API}/boards/search");

    s.get("workspace search", &format!("{workspace_search}?q=zebra"))
        .user(&alice)
        .await;
    s.get(
        "workspace search: other user",
        &format!("{workspace_search}?q=zebra"),
    )
    .user(&bob)
    .await;
    s.get(
        "workspace search: no match",
        &format!("{workspace_search}?q=giraffe"),
    )
    .user(&alice)
    .await;
    s.get(
        "workspace search: empty term",
        &format!("{workspace_search}?q="),
    )
    .user(&alice)
    .await;
    s.get("workspace search: no term", &workspace_search)
        .user(&alice)
        .await;
    s.get(
        "workspace search: case",
        &format!("{workspace_search}?q=ZEBRA"),
    )
    .user(&alice)
    .await;
    s.get(
        "workspace search: by field",
        &format!("{workspace_search}?q=zebra&field=title"),
    )
    .user(&alice)
    .await;
    s.get(
        "workspace search: bad field",
        &format!("{workspace_search}?q=zebra&field=nope"),
    )
    .user(&alice)
    .await;
    s.get(
        "workspace search: anonymous",
        &format!("{workspace_search}?q=zebra"),
    )
    .await;
    s.get("all search: gone", &format!("{API}/boards/search?q=zebra"))
        .user(&alice)
        .await;
    s.get(
        "all search: gone, empty term",
        &format!("{API}/boards/search?q="),
    )
    .user(&alice)
    .await;
    s.get(
        "linkable search",
        &format!("{workspace_search}/linkable?q=zebra"),
    )
    .user(&alice)
    .await;
    s.get(
        "channels",
        &format!("{API}/teams/{ROOT_WORKSPACE_ID}/channels?search=x"),
    )
    .user(&alice)
    .await;
    s.get(
        "channel",
        &format!("{API}/teams/{ROOT_WORKSPACE_ID}/channels/some-channel"),
    )
    .user(&alice)
    .await;
    s.get("statistics", &format!("{API}/statistics"))
        .user(&alice)
        .await;
    s.get("compliance boards", &format!("{API}/admin/boards"))
        .user(&alice)
        .await;
    s.get(
        "compliance boards history",
        &format!("{API}/admin/boards_history?modified_since=1"),
    )
    .user(&alice)
    .await;
    s.get(
        "compliance blocks history",
        &format!("{API}/admin/blocks_history?modified_since=1"),
    )
    .user(&alice)
    .await;
}
