//! Scenarios: boards, board members, and the combined boards-and-blocks routes.
#![expect(
    clippy::too_many_lines,
    reason = "a scenario is one request sequence; cutting it up would hide the order it pins"
)]

use serde_json::json;

use crate::common::ROOT_WORKSPACE_ID;
use crate::fixtures::{id_of, new_board, new_card};
use crate::harness::ROOT_API;
use crate::harness::{API, Session};

pub(crate) async fn boards(s: &mut Session) {
    let [alice, bob] = s.users("boards", ["alice", "bob"]).await;
    let boards = format!("{ROOT_API}/boards");
    s.post("create: no visibility", &boards)
        .json(json!({"workspaceId": ROOT_WORKSPACE_ID, "title": "x"}))
        .user(&alice)
        .await;
    s.post("create: bad visibility", &boards)
        .json(json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "X", "title": "x"}))
        .user(&alice)
        .await;
    s.post("create: no workspace", &boards)
        .json(json!({"visibility": "private", "title": "x"}))
        .user(&alice)
        .await;
    s.post("create: without session", &boards)
        .json(json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "private"}))
        .await;
    s.post("create: malformed body", &boards)
        .raw(b"{nope")
        .user(&alice)
        .await;
    let board = new_board(
        s,
        &alice,
        "Roadmap",
        "private",
        json!({
            "description": "plans",
            "icon": "🎯",
            "showDescription": true,
            "properties": {"color": "blue"},
            "cardProperties": [{"id": "prop1", "name": "Status", "type": "select", "options": []}],
        }),
    )
    .await;
    let open_board = new_board(s, &alice, "Open board", "open", json!({})).await;
    let path = format!("{ROOT_API}/containers/{board}");
    let open_path = format!("{ROOT_API}/containers/{open_board}");
    let unknown = format!("{ROOT_API}/containers/unknown-board");

    s.get("get", &path).user(&alice).await;
    s.get("get: non-member, private", &path).user(&bob).await;
    s.get("get: non-member, open", &open_path).user(&bob).await;
    s.get("get: anonymous", &path).await;
    s.get("get: unknown board", &unknown).user(&alice).await;
    s.get("list: member", &format!("{ROOT_API}/boards"))
        .user(&alice)
        .await;
    s.get("list: other user", &format!("{ROOT_API}/boards"))
        .user(&bob)
        .await;
    s.get("metadata", &format!("{path}/metadata"))
        .user(&alice)
        .await;

    s.patch("patch", &path)
        .json(json!({
            "title": "Roadmap 2", "description": "new plans", "icon": "🚀", "showDescription": false,
            "updatedProperties": {"color": "red", "size": 3}, "deletedProperties": [],
            "updatedCardProperties": [{"id": "prop2", "name": "Owner", "type": "person", "options": []}],
            "deletedCardProperties": [],
        }))
        .user(&alice)
        .await;
    s.patch("patch: delete properties", &path)
        .json(json!({"deletedProperties": ["size"], "deletedCardProperties": ["prop1"]}))
        .user(&alice)
        .await;
    s.patch("patch: non-member", &path)
        .json(json!({"title": "hijack"}))
        .user(&bob)
        .await;
    s.patch("patch: bad visibility", &path)
        .json(json!({"visibility": "Z"}))
        .user(&alice)
        .await;
    s.patch("patch: to open", &path)
        .json(json!({"visibility": "open"}))
        .user(&alice)
        .await;
    s.patch("patch: back to private", &path)
        .json(json!({"visibility": "private"}))
        .user(&alice)
        .await;
    s.patch("patch: minimum role", &open_path)
        .json(json!({"minimumRole": "editor"}))
        .user(&alice)
        .await;
    s.patch("patch: unknown board", &unknown)
        .json(json!({"title": "x"}))
        .user(&alice)
        .await;
    s.patch("patch: malformed body", &path)
        .raw(b"[")
        .user(&alice)
        .await;
    s.get("get after patches", &path).user(&alice).await;

    new_card(
        s,
        &alice,
        &board,
        "A card",
        json!({"icon": "x"}),
        "insert a card",
    )
    .await;
    let duplicate = s
        .post("duplicate", &format!("{path}/duplicate"))
        .user(&alice)
        .await;
    s.post(
        "duplicate as template",
        &format!("{path}/duplicate?asTemplate=true"),
    )
    .user(&alice)
    .await;
    s.post("duplicate: non-member", &format!("{path}/duplicate"))
        .user(&bob)
        .await;
    s.post("duplicate: unknown board", &format!("{unknown}/duplicate"))
        .user(&alice)
        .await;
    let copy = id_of(&duplicate.json()["containers"][0]);
    s.get(
        "cards of the copy",
        &format!("{ROOT_API}/boards/{copy}/cards"),
    )
    .user(&alice)
    .await;

    s.delete("delete: non-member", &path).user(&bob).await;
    s.delete("delete", &path).user(&alice).await;
    s.get("get deleted", &path).user(&alice).await;
    s.delete("delete again", &path).user(&alice).await;
    s.post("undelete: other user", &format!("{path}/restore"))
        .user(&bob)
        .await;
    s.post("undelete", &format!("{path}/restore"))
        .user(&alice)
        .await;
    s.get("get undeleted", &path).user(&alice).await;
    s.get(
        "cards after undelete",
        &format!("{ROOT_API}/boards/{board}/cards"),
    )
    .user(&alice)
    .await;
    s.get("list at the end", &format!("{ROOT_API}/boards"))
        .user(&alice)
        .await;
}

pub(crate) async fn members(s: &mut Session) {
    let boards_api = format!("{ROOT_API}/boards");
    let containers_api = format!("{ROOT_API}/containers");
    let [alice, bob, carol] = s.users("members", ["alice", "bob", "carol"]).await;
    let board = new_board(s, &alice, "Workspace board", "private", json!({})).await;
    let open_board = new_board(s, &alice, "Open board", "open", json!({})).await;
    let bob_id = s.user_id(&bob).await;
    let carol_id = s.user_id(&carol).await;
    let alice_id = s.user_id(&alice).await;
    let members = format!("{containers_api}/{board}/members");
    let open_path = format!("{containers_api}/{open_board}");

    s.get("list", &members).user(&alice).await;
    s.get("list: non-member", &members).user(&bob).await;
    s.post("add: non-member adds", &members)
        .json(json!({"containerId": board, "userId": carol_id, "role": "editor"}))
        .user(&bob)
        .await;
    s.post("add editor", &members)
        .json(json!({"containerId": board, "userId": bob_id, "role": "editor"}))
        .user(&alice)
        .await;
    s.post("add: no user", &members)
        .json(json!({"containerId": board}))
        .user(&alice)
        .await;
    s.post("add: unknown user", &members)
        .json(json!({"containerId": board, "userId": "unknown-user", "role": "viewer"}))
        .user(&alice)
        .await;
    s.post("add: editor adds", &members)
        .json(json!({"containerId": board, "userId": carol_id, "role": "viewer"}))
        .user(&bob)
        .await;
    s.post("add viewer", &members)
        .json(json!({"containerId": board, "userId": carol_id, "role": "viewer"}))
        .user(&alice)
        .await;
    s.post("add: already a member", &members)
        .json(json!({"containerId": board, "userId": carol_id, "role": "admin"}))
        .user(&alice)
        .await;
    s.get("list after adding", &members).user(&bob).await;
    s.get(
        "memberships of the editor",
        &format!("{API}/users/me/memberships"),
    )
    .user(&bob)
    .await;

    s.put("update: to commenter", &format!("{members}/{carol_id}"))
        .json(json!({"containerId": board, "userId": carol_id, "role": "commenter"}))
        .user(&alice)
        .await;
    s.put("update: by editor", &format!("{members}/{carol_id}"))
        .json(json!({"containerId": board, "userId": carol_id, "role": "admin"}))
        .user(&bob)
        .await;
    s.put(
        "update: demote the last admin",
        &format!("{members}/{alice_id}"),
    )
    .json(json!({"containerId": board, "userId": alice_id, "role": "editor"}))
    .user(&alice)
    .await;
    s.put("update: unknown member", &format!("{members}/unknown-user"))
        .json(json!({"containerId": board, "userId": "unknown-user", "role": "editor"}))
        .user(&alice)
        .await;

    s.delete("delete: by editor", &format!("{members}/{carol_id}"))
        .user(&bob)
        .await;
    s.delete("delete: the last admin", &format!("{members}/{alice_id}"))
        .user(&alice)
        .await;
    s.delete("delete", &format!("{members}/{carol_id}"))
        .user(&alice)
        .await;
    s.delete("delete: not a member", &format!("{members}/{carol_id}"))
        .user(&alice)
        .await;

    s.post(
        "join: private board",
        &format!("{containers_api}/{board}/join"),
    )
    .user(&carol)
    .await;
    s.post("join: open board", &format!("{open_path}/join"))
        .user(&carol)
        .await;
    s.post("join: again", &format!("{open_path}/join"))
        .user(&carol)
        .await;
    s.post(
        "join: unknown board",
        &format!("{containers_api}/unknown-board/join"),
    )
    .user(&carol)
    .await;
    s.post("leave", &format!("{open_path}/leave"))
        .user(&carol)
        .await;
    s.post("leave: not a member", &format!("{open_path}/leave"))
        .user(&carol)
        .await;
    s.post("leave: the last admin", &format!("{open_path}/leave"))
        .user(&alice)
        .await;
    s.get("members at the end", &members).user(&alice).await;

    // -- the minimum role ----------------------------------------------------------------
    // What joining gives, and what somebody gives who adds a member without managing the
    // board's roles. A board always has one; a new board has editor.
    let role_board = new_board(s, &alice, "Role board", "open", json!({})).await;
    let role_path = format!("{containers_api}/{role_board}");
    let role_members = format!("{role_path}/members");
    s.patch("minimum role: none", &role_path)
        .json(json!({"minimumRole": ""}))
        .user(&alice)
        .await;
    s.patch("minimum role: admin", &role_path)
        .json(json!({"minimumRole": "admin"}))
        .user(&alice)
        .await;
    s.post(
        "create a board with no minimum role",
        &format!("{ROOT_API}/boards"),
    )
    .json(json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "open", "minimumRole": ""}))
    .user(&alice)
    .await;
    s.patch("minimum role: commenter", &role_path)
        .json(json!({"minimumRole": "commenter"}))
        .user(&alice)
        .await;
    s.post(
        "add below the minimum role: admin adds a viewer",
        &role_members,
    )
    .json(json!({"containerId": role_board, "userId": bob_id, "role": "viewer"}))
    .user(&alice)
    .await;
    let role_card = new_card(s, &alice, &role_board, "Card", json!({}), "create a card").await;
    s.post(
        "a viewer on the board cannot comment",
        &format!("{boards_api}/{role_board}/cards/{role_card}/comments"),
    )
    .json(json!({"body": {"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "no"}]},
    ]}}))
    .user(&bob)
    .await;
    s.post(
        "join again: raised to the minimum role",
        &format!("{role_path}/join"),
    )
    .user(&bob)
    .await;
    s.post(
        "join once more: nothing changes",
        &format!("{role_path}/join"),
    )
    .user(&bob)
    .await;
    s.post(
        "join: a new member gets the minimum role",
        &format!("{role_path}/join"),
    )
    .user(&carol)
    .await;
    s.put("make an editor", &format!("{role_members}/{carol_id}"))
        .json(json!({"role": "editor"}))
        .user(&alice)
        .await;
    s.post(
        "join above the minimum role: not lowered",
        &format!("{role_path}/join"),
    )
    .user(&carol)
    .await;
    s.post(
        "join: the admin stays the admin",
        &format!("{role_path}/join"),
    )
    .user(&alice)
    .await;
    s.get("members of the role board", &role_members)
        .user(&alice)
        .await;
}
