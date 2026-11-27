//! What a user cannot see answers as what does not exist.
//!
//! For every route that names a board, a block, a card, a file, a category or a
//! subscription, a user who has no business with it gets the same answer as for an
//! identifier nobody has: the same status, the same code, the same message. Nothing in
//! an answer tells the two apart, so nothing is learned about what others have.
//!
//! "An identifier nobody has" is asked twice: with a well-formed identifier that names
//! nothing, and with text that is no identifier at all. What cannot be an identifier is
//! not there either, and it gets the same answer again.

use fvoci_model::{Block, BlockType, Category, ContainerVisibility, new_id};
use reqwest::multipart::{Form, Part};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::TEST_WS;
use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID, TestServer};

/// What exists and is hidden from the second user, and what stands in for it in the
/// request that names nothing.
struct Hidden {
    th: TestServer,
    /// The identifiers of what the first user has, each with one nobody has and with
    /// text that is no identifier.
    board: [String; 3],
    card: [String; 3],
    file: [String; 3],
    category: [String; 3],
    owner: [String; 3],
    /// The second user, who sees none of it.
    outsider: String,
    /// What the second user has of their own: a private board with a card and a block
    /// below it, and a category.
    own_board: String,
    own_card: String,
    own_category: String,
}

impl Hidden {
    #[expect(
        clippy::too_many_lines,
        reason = "one fixture: everything of two users that the tables ask about"
    )]
    async fn start() -> Self {
        let th = Setup::local_mode().start().await.init_basic().await;
        let owner = th.client.me().await;
        let outsider = th.client2.me().await.id;
        let board = th
            .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
            .await;
        let block = |block_type: BlockType, parent_id: &str| Block {
            id: new_id(),
            container_id: board.id.clone(),
            parent_id: parent_id.to_owned(),
            block_type,
            created_at: 1,
            updated_at: 1,
            ..Block::default()
        };
        let mut created =
            crate::blocks::insert(&th.client, &board.id, &[block(BlockType::Card, "")]).await;
        let card = created.pop().expect("the card is created");
        let mut created: Vec<Block> = th
            .client
            .insert_blocks(&board.id, &[block(BlockType::Attachment, &card.id)])
            .await
            .created_json();
        created.pop().expect("the text is created");

        let part = Part::bytes(b"hidden".to_vec()).file_name("hidden.txt");
        let request = th
            .client
            .request(
                Method::POST,
                &format!("{TEST_WS}/containers/{}/files", board.id),
            )
            .multipart(Form::new().part("file", part));
        let uploaded = Client::send(request).await;
        uploaded.check_created();
        let file = uploaded.value()["fileId"]
            .as_str()
            .expect("the upload names the file")
            .to_owned();

        // Listing creates the default category; then one of the owner's own.
        th.client
            .get(&format!("{TEST_WS}/categories"))
            .await
            .check_ok();
        let category: Category = th
            .client
            .post(
                &format!("{TEST_WS}/categories"),
                &json!({"name": "Hidden", "userId": owner.id, "workspaceId": TEST_WORKSPACE_ID}),
            )
            .await
            .created_json();
        th.client
            .post(
                &format!("{TEST_WS}/subscriptions"),
                &json!({
                    "blockType": "card",
                    "blockId": card.id,
                    "userId": owner.id,
                }),
            )
            .await
            .check_created();
        th.client
            .post(
                &format!("{TEST_WS}/containers/{}/public-link", board.id),
                &json!({"enabled": true}),
            )
            .await
            .check_ok();

        // What the outsider has of their own, to ask about hidden things through.
        let own_board = th
            .client2
            .create_board(&fvoci_model::Container {
                workspace_id: TEST_WORKSPACE_ID.to_owned(),
                visibility: ContainerVisibility::Private,
                ..fvoci_model::Container::default()
            })
            .await
            .created_json::<fvoci_model::Container>();
        let own_block = |block_type: BlockType, parent_id: &str| Block {
            id: new_id(),
            container_id: own_board.id.clone(),
            parent_id: parent_id.to_owned(),
            block_type,
            created_at: 1,
            updated_at: 1,
            ..Block::default()
        };
        let mut created = crate::blocks::insert(
            &th.client2,
            &own_board.id,
            &[own_block(BlockType::Card, "")],
        )
        .await;
        let own_card = created.pop().expect("the card is created");
        let mut created: Vec<Block> = th
            .client2
            .insert_blocks(
                &own_board.id,
                &[own_block(BlockType::Attachment, &own_card.id)],
            )
            .await
            .created_json();
        created.pop().expect("the text is created");
        th.client2
            .get(&format!("{TEST_WS}/categories"))
            .await
            .check_ok();
        let own_category: Category = th
            .client2
            .post(
                &format!("{TEST_WS}/categories"),
                &json!({"name": "Own", "userId": outsider, "workspaceId": TEST_WORKSPACE_ID}),
            )
            .await
            .created_json();

        Self {
            own_board: own_board.id,
            own_card: own_card.id,
            own_category: own_category.id,
            board: [board.id, new_id(), "no-board".to_owned()],
            card: [card.id, new_id(), "no-card".to_owned()],
            file: [
                file,
                format!("{}.txt", fvoci_server::auth::secret::new_secret()),
                "no-file.txt".to_owned(),
            ],
            category: [category.id, new_id(), "no-category".to_owned()],
            owner: [owner.id, new_id(), "no-user".to_owned()],
            outsider,
            th,
        }
    }
}

/// One request, said once for what is hidden, once for what does not exist, and once
/// for what cannot exist.
struct Case {
    method: Method,
    /// The path, with the existing identifiers or the missing ones.
    path: fn(&Hidden, usize) -> String,
    /// The JSON body, likewise; `null` for a request without one.
    body: fn(&Hidden, usize) -> Value,
}

fn no_body(_: &Hidden, _: usize) -> Value {
    Value::Null
}

fn title(_: &Hidden, _: usize) -> Value {
    json!({"title": "by an outsider"})
}

fn member(_: &Hidden, _: usize) -> Value {
    json!({"userId": "somebody", "role": "editor"})
}

/// The path of a category of the test workspace, and of what follows it.
fn category_path(category: &str, rest: &str) -> String {
    format!("{TEST_WS}/categories/{category}{rest}")
}

/// The path of a board in a category of the test workspace, and of what follows it.
fn filed_path(category: &str, board: &str, rest: &str) -> String {
    category_path(category, &format!("/containers/{board}{rest}"))
}

/// Every route that names something of the hidden owner's: [`board_cases`], and what is
/// on the board or of the owner ([`content_cases`]).
fn cases() -> Vec<Case> {
    let mut cases = board_cases();
    cases.extend(content_cases());
    cases
}

/// A case of the table.
fn case(
    method: Method,
    path: fn(&Hidden, usize) -> String,
    body: fn(&Hidden, usize) -> Value,
) -> Case {
    Case { method, path, body }
}

/// The routes of the board itself, its members and its sharing.
fn board_cases() -> Vec<Case> {
    vec![
        // The board itself.
        case(
            Method::GET,
            |h, i| format!("{TEST_WS}/containers/{}", h.board[i]),
            no_body,
        ),
        case(
            Method::PATCH,
            |h, i| format!("{TEST_WS}/containers/{}", h.board[i]),
            title,
        ),
        case(
            Method::PUT,
            |h, i| format!("{TEST_WS}/boards/{}/property-order", h.board[i]),
            |_, _| json!([]),
        ),
        case(
            Method::DELETE,
            |h, i| format!("{TEST_WS}/containers/{}", h.board[i]),
            no_body,
        ),
        case(
            Method::POST,
            |h, i| format!("{TEST_WS}/containers/{}/duplicate", h.board[i]),
            no_body,
        ),
        case(
            Method::POST,
            |h, i| format!("{TEST_WS}/containers/{}/restore", h.board[i]),
            no_body,
        ),
        case(
            Method::GET,
            |h, i| format!("{TEST_WS}/containers/{}/archive/export", h.board[i]),
            no_body,
        ),
        // Its members.
        case(
            Method::GET,
            |h, i| format!("{TEST_WS}/containers/{}/members", h.board[i]),
            no_body,
        ),
        case(
            Method::POST,
            |h, i| format!("{TEST_WS}/containers/{}/members", h.board[i]),
            member,
        ),
        case(
            Method::PUT,
            |h, i| format!("{TEST_WS}/containers/{}/members/{}", h.board[i], h.owner[i]),
            member,
        ),
        case(
            Method::DELETE,
            |h, i| format!("{TEST_WS}/containers/{}/members/{}", h.board[i], h.owner[i]),
            no_body,
        ),
        case(
            Method::POST,
            |h, i| format!("{TEST_WS}/containers/{}/join", h.board[i]),
            no_body,
        ),
        case(
            Method::POST,
            |h, i| format!("{TEST_WS}/containers/{}/leave", h.board[i]),
            no_body,
        ),
        // Its sharing.
        case(
            Method::GET,
            |h, i| format!("{TEST_WS}/containers/{}/public-link", h.board[i]),
            no_body,
        ),
        case(
            Method::POST,
            |h, i| format!("{TEST_WS}/containers/{}/public-link", h.board[i]),
            |_, _| json!({"enabled": false}),
        ),
    ]
}

/// The routes of the board's blocks, cards and files, the combined requests, the
/// owner's subscriptions and category.
#[expect(
    clippy::too_many_lines,
    reason = "a table with one entry per route, not logic"
)]
fn content_cases() -> Vec<Case> {
    vec![
        // Its blocks.
        case(
            Method::GET,
            |h, i| format!("{TEST_WS}/boards/{}/blocks", h.board[i]),
            no_body,
        ),
        case(
            Method::GET,
            |h, i| {
                format!(
                    "{TEST_WS}/boards/{}/blocks?blockId={}",
                    h.board[i], h.card[i]
                )
            },
            no_body,
        ),
        case(
            Method::POST,
            |h, i| format!("{TEST_WS}/boards/{}/blocks", h.board[i]),
            |h, i| {
                json!([{
                    "containerId": h.board[i], "type": "view",
                }])
            },
        ),
        case(
            Method::PATCH,
            |h, i| format!("{TEST_WS}/boards/{}/blocks", h.board[i]),
            |h, i| json!({"blockIds": [h.card[i]], "blockPatches": [{"title": "x"}]}),
        ),
        case(
            Method::PATCH,
            |h, i| format!("{TEST_WS}/boards/{}/blocks/{}", h.board[i], h.card[i]),
            title,
        ),
        case(
            Method::DELETE,
            |h, i| format!("{TEST_WS}/boards/{}/blocks/{}", h.board[i], h.card[i]),
            no_body,
        ),
        case(
            Method::POST,
            |h, i| {
                format!(
                    "{TEST_WS}/boards/{}/blocks/{}/restore",
                    h.board[i], h.card[i]
                )
            },
            no_body,
        ),
        case(
            Method::POST,
            |h, i| {
                format!(
                    "{TEST_WS}/boards/{}/blocks/{}/duplicate",
                    h.board[i], h.card[i]
                )
            },
            no_body,
        ),
        case(
            Method::GET,
            |h, i| {
                format!(
                    "{TEST_WS}/boards/{}/blocks/{}/versions",
                    h.board[i], h.card[i]
                )
            },
            no_body,
        ),
        case(
            Method::GET,
            |h, i| format!("{TEST_WS}/boards/{}/trash/cards", h.board[i]),
            no_body,
        ),
        // Its cards, by the board and by themselves.
        case(
            Method::GET,
            |h, i| format!("{TEST_WS}/boards/{}/cards", h.board[i]),
            no_body,
        ),
        case(
            Method::POST,
            |h, i| format!("{TEST_WS}/boards/{}/cards", h.board[i]),
            title,
        ),
        case(
            Method::GET,
            |h, i| format!("{TEST_WS}/cards/{}", h.card[i]),
            no_body,
        ),
        case(
            Method::PATCH,
            |h, i| format!("{TEST_WS}/cards/{}", h.card[i]),
            title,
        ),
        // Its files.
        case(
            Method::GET,
            |h, i| format!("{TEST_WS}/containers/{}/files/{}", h.board[i], h.file[i]),
            no_body,
        ),
        case(
            Method::GET,
            |h, i| {
                format!(
                    "{TEST_WS}/containers/{}/files/{}/info",
                    h.board[i], h.file[i]
                )
            },
            no_body,
        ),
        // Both kinds of combined request.
        case(
            Method::PATCH,
            |_, _| format!("{TEST_WS}/containers-and-blocks"),
            |h, i| json!({"containerIds": [h.board[i]], "containerPatches": [{"title": "x"}]}),
        ),
        case(
            Method::DELETE,
            |_, _| format!("{TEST_WS}/containers-and-blocks"),
            |h, i| json!({"containers": [h.board[i]]}),
        ),
        // A subscription to one of its blocks, and the owner's subscriptions.
        case(
            Method::POST,
            |_, _| format!("{TEST_WS}/subscriptions"),
            |h, i| {
                json!({
                    "blockType": "card", "blockId": h.card[i],
                    "userId": h.outsider,
                })
            },
        ),
        case(
            Method::GET,
            |h, i| format!("{TEST_WS}/subscriptions/{}", h.owner[i]),
            no_body,
        ),
        case(
            Method::DELETE,
            |h, i| format!("{TEST_WS}/subscriptions/{}/{}", h.card[i], h.owner[i]),
            no_body,
        ),
        // The owner's category.
        case(
            Method::PUT,
            |h, i| category_path(&h.category[i], ""),
            |h, i| json!({"id": h.category[i], "name": "x", "workspaceId": TEST_WORKSPACE_ID}),
        ),
        case(
            Method::DELETE,
            |h, i| category_path(&h.category[i], ""),
            no_body,
        ),
        case(
            Method::PUT,
            |h, i| category_path(&h.category[i], "/container-order"),
            |_, _| json!([]),
        ),
        case(
            Method::POST,
            |h, i| filed_path(&h.category[i], &h.board[i], ""),
            no_body,
        ),
        case(
            Method::PUT,
            |h, i| filed_path(&h.category[i], &h.board[i], "/hide"),
            no_body,
        ),
    ]
}

/// Requests that name something of the asker's own together with something hidden: the
/// places where one part of a request passes every check and the other part must not
/// be given away by the answer.
#[expect(
    clippy::too_many_lines,
    reason = "a table with one entry per route, not logic"
)]
fn mixed_cases() -> Vec<Case> {
    let case = |method: Method,
                path: fn(&Hidden, usize) -> String,
                body: fn(&Hidden, usize) -> Value| Case { method, path, body };
    vec![
        // A hidden block through the asker's own board.
        case(
            Method::GET,
            |h, i| {
                format!(
                    "{TEST_WS}/boards/{}/blocks?blockId={}",
                    h.own_board, h.card[i]
                )
            },
            no_body,
        ),
        case(
            Method::PATCH,
            |h, i| format!("{TEST_WS}/boards/{}/blocks/{}", h.own_board, h.card[i]),
            title,
        ),
        case(
            Method::DELETE,
            |h, i| format!("{TEST_WS}/boards/{}/blocks/{}", h.own_board, h.card[i]),
            no_body,
        ),
        case(
            Method::POST,
            |h, i| {
                format!(
                    "{TEST_WS}/boards/{}/blocks/{}/restore",
                    h.own_board, h.card[i]
                )
            },
            no_body,
        ),
        case(
            Method::POST,
            |h, i| {
                format!(
                    "{TEST_WS}/boards/{}/blocks/{}/duplicate",
                    h.own_board, h.card[i]
                )
            },
            no_body,
        ),
        case(
            Method::GET,
            |h, i| {
                format!(
                    "{TEST_WS}/boards/{}/blocks/{}/versions",
                    h.own_board, h.card[i]
                )
            },
            no_body,
        ),
        case(
            Method::PATCH,
            |h, _| format!("{TEST_WS}/boards/{}/blocks", h.own_board),
            |h, i| {
                json!({
                    "blockIds": [h.own_card, h.card[i]],
                    "blockPatches": [{"title": "x"}, {"title": "y"}],
                })
            },
        ),
        // The asker's own board together with a hidden block, or a hidden board.
        case(
            Method::PATCH,
            |_, _| format!("{TEST_WS}/containers-and-blocks"),
            |h, i| {
                json!({
                    "containerIds": [h.own_board], "containerPatches": [{"title": "x"}],
                    "blockIds": [h.card[i]], "blockPatches": [{"title": "y"}],
                })
            },
        ),
        case(
            Method::PATCH,
            |_, _| format!("{TEST_WS}/containers-and-blocks"),
            |h, i| {
                json!({
                    "containerIds": [h.own_board, h.board[i]],
                    "containerPatches": [{"title": "x"}, {"title": "y"}],
                })
            },
        ),
        case(
            Method::DELETE,
            |_, _| format!("{TEST_WS}/containers-and-blocks"),
            |h, i| json!({"containers": [h.own_board], "blocks": [h.card[i]]}),
        ),
        // Blocks for the asker's own board, with files to copy from a hidden board.
        case(
            Method::POST,
            |h, i| {
                format!(
                    "{TEST_WS}/boards/{}/blocks?sourceBoardId={}",
                    h.own_board, h.board[i]
                )
            },
            |h, i| {
                json!([{
                    "containerId": h.own_board, "type": "attachment", "createdAt": 1, "updatedAt": 1,
                    "fields": {"fileId": h.file[i]},
                }])
            },
        ),
        // A hidden board in the asker's own category.
        case(
            Method::POST,
            |h, i| filed_path(&h.own_category, &h.board[i], ""),
            no_body,
        ),
        case(
            Method::PUT,
            |h, i| filed_path(&h.own_category, &h.board[i], "/hide"),
            no_body,
        ),
        case(
            Method::PUT,
            |h, i| filed_path(&h.own_category, &h.board[i], "/unhide"),
            no_body,
        ),
        // A new category that names the identifier of somebody's category.
        case(
            Method::POST,
            |_, _| format!("{TEST_WS}/categories"),
            |h, i| {
                json!({
                    "id": h.category[i], "name": "x",
                    "userId": h.outsider, "workspaceId": TEST_WORKSPACE_ID,
                })
            },
        ),
        // A file of the hidden board through the asker's own board.
        case(
            Method::GET,
            |h, i| format!("{TEST_WS}/containers/{}/files/{}", h.own_board, h.file[i]),
            no_body,
        ),
        case(
            Method::GET,
            |h, i| {
                format!(
                    "{TEST_WS}/containers/{}/files/{}/info",
                    h.own_board, h.file[i]
                )
            },
            no_body,
        ),
    ]
}

/// The parts of an answer a client could tell two requests apart by.
fn told(response: &ApiResponse) -> (StatusCode, Value) {
    if response.body.is_empty() {
        // An answer without a body tells its status and nothing else.
        return (response.status, Value::Null);
    }
    let mut body = response.value();
    if let Some(body) = body.as_object_mut() {
        // Every request has its own identifier.
        body.remove("requestId");
        body.remove("instance");
    }
    (response.status, body)
}

async fn ask(client: &Client, hidden: &Hidden, case: &Case, which: usize) -> ApiResponse {
    let path = (case.path)(hidden, which);
    let request = client.request(case.method.clone(), &path);
    let request = match (case.body)(hidden, which) {
        Value::Null => request,
        body => request.json(&body),
    };
    Client::send(request).await
}

#[tokio::test]
async fn what_a_user_cannot_see_answers_as_what_does_not_exist() {
    let hidden = Hidden::start().await;
    let outsider = &hidden.th.client2;

    for case in cases() {
        let path = (case.path)(&hidden, 0);
        let existing = ask(outsider, &hidden, &case, 0).await;
        let missing = ask(outsider, &hidden, &case, 1).await;
        let malformed = ask(outsider, &hidden, &case, 2).await;

        assert_eq!(
            existing.status,
            StatusCode::NOT_FOUND,
            "{} {path}: {}",
            case.method,
            existing.text()
        );
        assert_eq!(
            told(&existing),
            told(&missing),
            "{} {path}: what is hidden and what is missing answer alike",
            case.method
        );
        assert_eq!(
            told(&missing),
            told(&malformed),
            "{} {}: what is missing and what is no identifier answer alike",
            case.method,
            (case.path)(&hidden, 2)
        );
        assert_eq!(
            existing.value()["type"],
            json!("/problems/not-found"),
            "{} {path}",
            case.method
        );
    }

    // None of it changed anything: the owner still has it all.
    let owner = &hidden.th.client;
    owner
        .get(&format!("{TEST_WS}/containers/{}", hidden.board[0]))
        .await
        .check_ok();
    let blocks: Vec<Block> = owner
        .get(&format!("{TEST_WS}/boards/{}/blocks", hidden.board[0]))
        .await
        .ok_json();
    assert_eq!(blocks.len(), 2, "the card and its text");
    let members: Vec<Value> = owner
        .get(&format!("{TEST_WS}/containers/{}/members", hidden.board[0]))
        .await
        .ok_json();
    assert_eq!(members.len(), 1, "the owner alone");
    let subscriptions: Vec<Value> = owner
        .get(&format!("{TEST_WS}/subscriptions/{}", hidden.owner[0]))
        .await
        .ok_json();
    assert_eq!(subscriptions.len(), 1, "the owner's subscription");
    hidden.th.teardown().await;
}

/// The same where a request names something of the asker's own next to something
/// hidden: the request is refused, and the same way as for an identifier nobody has.
#[tokio::test]
async fn a_hidden_thing_next_to_an_own_one_answers_as_one_that_does_not_exist() {
    let hidden = Hidden::start().await;
    let outsider = &hidden.th.client2;

    for case in mixed_cases() {
        let path = (case.path)(&hidden, 0);
        let existing = ask(outsider, &hidden, &case, 0).await;
        let missing = ask(outsider, &hidden, &case, 1).await;
        let malformed = ask(outsider, &hidden, &case, 2).await;

        assert!(
            existing.status.is_client_error(),
            "{} {path} is refused: {} {}",
            case.method,
            existing.status,
            existing.text()
        );
        assert_eq!(
            told(&existing),
            told(&missing),
            "{} {path}: what is hidden and what is missing answer alike",
            case.method
        );
        assert_eq!(
            told(&missing),
            told(&malformed),
            "{} {}: what is missing and what is no identifier answer alike",
            case.method,
            (case.path)(&hidden, 2)
        );
    }

    // The outsider's own things are as they were, and so are the owner's.
    let own: Vec<Block> = outsider
        .get(&format!("{TEST_WS}/boards/{}/blocks", hidden.own_board))
        .await
        .ok_json();
    assert_eq!(own.len(), 2, "the outsider's card and text, nothing added");
    assert!(
        own.iter().all(|block| block.title.is_empty()),
        "and nothing patched: {own:?}"
    );
    let blocks: Vec<Block> = hidden
        .th
        .client
        .get(&format!("{TEST_WS}/boards/{}/blocks", hidden.board[0]))
        .await
        .ok_json();
    assert_eq!(blocks.len(), 2, "the owner's card and text");
    hidden.th.teardown().await;
}

/// Without a session the answer is the same for everything too: 401, whatever is named.
#[tokio::test]
async fn without_a_session_nothing_is_told_either() {
    let hidden = Hidden::start().await;
    let anonymous = hidden.th.anonymous();

    for case in cases() {
        let path = (case.path)(&hidden, 0);
        let existing = ask(&anonymous, &hidden, &case, 0).await;
        let missing = ask(&anonymous, &hidden, &case, 1).await;

        assert_eq!(
            existing.status,
            StatusCode::UNAUTHORIZED,
            "{} {path}: {}",
            case.method,
            existing.text()
        );
        assert_eq!(
            told(&existing),
            told(&missing),
            "{} {path}: what is there and what is missing answer alike",
            case.method
        );
    }
    hidden.th.teardown().await;
}

/// The routes of the organization and of a workspace's audit log that name something,
/// asked of the organization's administrator; `{}` stands for what is asked about.
fn organization_requests(org: &str) -> Vec<(Method, String, Value)> {
    vec![
        (Method::GET, "/w/{}/audit".to_owned(), Value::Null),
        (Method::GET, "/org/{}".to_owned(), Value::Null),
        (Method::GET, "/org/{}/workspaces".to_owned(), Value::Null),
        (
            Method::PATCH,
            format!("/org/{org}/workspaces/{{}}"),
            json!({"name": "x"}),
        ),
        (
            Method::DELETE,
            format!("/org/{org}/workspaces/{{}}"),
            Value::Null,
        ),
        (
            Method::POST,
            format!("/org/{org}/workspaces/{{}}/restore"),
            Value::Null,
        ),
        (
            Method::POST,
            format!("/org/{org}/workspaces/{{}}/join"),
            Value::Null,
        ),
        (
            Method::POST,
            format!("/org/{org}/workspaces/{{}}/leave"),
            Value::Null,
        ),
        (
            Method::PUT,
            format!("/org/{org}/members/{{}}/role"),
            json!({"role": "admin"}),
        ),
    ]
}

/// The routes of who is in a workspace and of the links that let others in; `{}`
/// stands for the identifier that is asked about, and `me` is a user of the server.
fn the_people_of_a_workspace(me: &str) -> Vec<(Method, String, Value)> {
    vec![
        (Method::POST, "/w/{}/invitations".to_owned(), Value::Null),
        (Method::GET, "/w/{}/invitations".to_owned(), Value::Null),
        (Method::GET, "/w/{}/members".to_owned(), Value::Null),
        (
            Method::PUT,
            format!("/w/{{}}/members/{me}"),
            json!({"role": "member"}),
        ),
        (Method::DELETE, format!("/w/{{}}/members/{me}"), Value::Null),
        (
            Method::DELETE,
            format!("/w/{{}}/invitations/{}", crate::common::UNKNOWN_ID),
            Value::Null,
        ),
        (
            Method::PUT,
            "/w/{}/guest-invitations".to_owned(),
            json!({"enabled": true}),
        ),
    ]
}

fn time_entry_requests() -> Vec<(Method, String, Value)> {
    vec![
        (
            Method::PATCH,
            "/users/me/time/entries/{}".to_owned(),
            json!({"note": "x"}),
        ),
        (
            Method::DELETE,
            "/users/me/time/entries/{}".to_owned(),
            Value::Null,
        ),
        (
            Method::GET,
            "/users/me/time/entries?blockId={}".to_owned(),
            Value::Null,
        ),
        (
            Method::GET,
            "/users/me/time/entries?containerId={}".to_owned(),
            Value::Null,
        ),
        (
            Method::POST,
            "/users/me/time/start".to_owned(),
            json!({"workspaceId": "{}", "kind": "work"}),
        ),
        (
            Method::POST,
            "/users/me/time/start".to_owned(),
            json!({"containerId": "{}", "kind": "work"}),
        ),
        (
            Method::POST,
            "/users/me/time/start".to_owned(),
            json!({"blockId": "{}", "kind": "work"}),
        ),
        (
            Method::POST,
            "/users/me/time/entries".to_owned(),
            json!({"workspaceId": "{}", "kind": "study", "startedAt": 1, "endedAt": 2}),
        ),
        (
            Method::POST,
            "/users/me/time/entries".to_owned(),
            json!({"blockId": "{}", "kind": "study", "startedAt": 1, "endedAt": 2}),
        ),
    ]
}

/// Text that is no identifier answers as an identifier of nothing, also where the asker
/// is allowed everything: on their own board, in their own workspace, about themselves. An
/// identifier column takes no such text, and no route may let that show: not as a 500,
/// and not as an answer that differs from "there is no such thing".
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "a table with one entry per route, not logic"
)]
async fn text_that_is_no_identifier_answers_as_an_identifier_of_nothing() {
    let hidden = Hidden::start().await;
    let owner = &hidden.th.client;
    let me = &hidden.owner[0];
    let board = &hidden.board[0];
    let card = &hidden.card[0];
    let category = &hidden.category[0];
    let workspace = TEST_WORKSPACE_ID;
    let workspaces: Vec<fvoci_model::UserWorkspace> =
        owner.get("/users/me/workspaces").await.ok_json();
    // The owner administers the organization.
    let org = &workspaces
        .first()
        .expect("the owner is in a workspace")
        .org_id;

    // `{}` stands for the identifier that is asked about.
    let mut requests: Vec<(Method, String, Value)> = vec![
        (Method::GET, "/w/{}".to_owned(), Value::Null),
        (Method::GET, "/w/{}/boards".to_owned(), Value::Null),
        (Method::GET, "/w/{}/templates".to_owned(), Value::Null),
        (Method::GET, "/w/{}/users".to_owned(), Value::Null),
        (Method::GET, "/w/{}/categories".to_owned(), Value::Null),
        (
            Method::GET,
            "/w/{}/boards/search?q=x".to_owned(),
            Value::Null,
        ),
        (Method::POST, "/w/{}/onboard".to_owned(), Value::Null),
        (Method::GET, format!("{TEST_WS}/users/{{}}"), Value::Null),
        (
            Method::GET,
            format!("{TEST_WS}/subscriptions/{{}}"),
            Value::Null,
        ),
        (
            Method::DELETE,
            format!("{TEST_WS}/subscriptions/{{}}/{me}"),
            Value::Null,
        ),
        (
            Method::DELETE,
            format!("{TEST_WS}/subscriptions/{card}/{{}}"),
            Value::Null,
        ),
        (Method::GET, format!("{TEST_WS}/cards/{{}}"), Value::Null),
        (
            Method::PATCH,
            format!("{TEST_WS}/cards/{{}}"),
            json!({"title": "x"}),
        ),
        (
            Method::GET,
            format!("{TEST_WS}/boards/{board}/blocks?blockId={{}}"),
            Value::Null,
        ),
        (
            Method::GET,
            format!("{TEST_WS}/boards/{board}/blocks?parentId={{}}"),
            Value::Null,
        ),
        (
            Method::PATCH,
            format!("{TEST_WS}/boards/{board}/blocks/{{}}"),
            json!({"title": "x"}),
        ),
        (
            Method::DELETE,
            format!("{TEST_WS}/boards/{board}/blocks/{{}}"),
            Value::Null,
        ),
        (
            Method::POST,
            format!("{TEST_WS}/boards/{board}/blocks/{{}}/restore"),
            Value::Null,
        ),
        (
            Method::POST,
            format!("{TEST_WS}/boards/{board}/blocks/{{}}/duplicate"),
            Value::Null,
        ),
        (
            Method::GET,
            format!("{TEST_WS}/boards/{board}/blocks/{{}}/versions"),
            Value::Null,
        ),
        (
            Method::PUT,
            format!("{TEST_WS}/containers/{board}/members/{{}}"),
            json!({"role": "editor"}),
        ),
        (
            Method::DELETE,
            format!("{TEST_WS}/containers/{board}/members/{{}}"),
            Value::Null,
        ),
        (
            Method::POST,
            format!("{TEST_WS}/containers/{board}/members"),
            json!({"userId": "{}", "role": "editor"}),
        ),
        (
            Method::POST,
            format!("{TEST_WS}/containers/{board}/duplicate?toWorkspace={{}}"),
            Value::Null,
        ),
        (
            Method::PUT,
            format!("/w/{workspace}/categories/{{}}"),
            json!({"id": "{}", "name": "x", "userId": me, "workspaceId": workspace}),
        ),
        (
            Method::DELETE,
            format!("/w/{workspace}/categories/{{}}"),
            Value::Null,
        ),
        (
            Method::POST,
            format!("/w/{workspace}/categories/{{}}/containers/{board}"),
            Value::Null,
        ),
        (
            Method::POST,
            format!("/w/{workspace}/categories/{category}/containers/{{}}"),
            Value::Null,
        ),
        (
            Method::PUT,
            format!("/w/{workspace}/categories/{category}/containers/{{}}/hide"),
            Value::Null,
        ),
        (
            Method::POST,
            format!("/content-blocks/{{}}/moveto/after/{card}"),
            Value::Null,
        ),
        (
            Method::POST,
            format!("/content-blocks/{card}/moveto/after/{{}}"),
            Value::Null,
        ),
        (
            Method::POST,
            format!("{TEST_WS}/subscriptions"),
            json!({"blockType": "card", "blockId": "{}", "userId": me}),
        ),
    ];
    requests.extend(the_people_of_a_workspace(me));
    requests.extend(organization_requests(org));
    requests.extend(time_entry_requests());

    let unknown = new_id();
    for (method, path, body) in requests {
        let ask = async |identifier: &str| {
            let path = path.replace("{}", identifier);
            let request = owner.request(method.clone(), &path);
            let request = if body.is_null() {
                request
            } else {
                let body: Value = serde_json::from_str(&body.to_string().replace("{}", identifier))
                    .expect("the body is JSON with any identifier in it");
                request.json(&body)
            };
            Client::send(request).await
        };
        let missing = ask(&unknown).await;
        assert_ne!(
            missing.status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{method} {path}: {}",
            missing.text()
        );
        // (An empty value is not in the list: an empty path segment is another route,
        // and an empty member of a body is a missing one.)
        for no_identifier in [
            "nope",
            "0",
            "01890A5D-AC96-774B-BCCE-B302099A8057",
            "00000000-0000-0000-0000-000000000000",
            "' OR 1=1 --",
        ] {
            let malformed = ask(no_identifier).await;
            assert_eq!(
                told(&missing),
                told(&malformed),
                "{method} {path} with {no_identifier:?}: what is no identifier answers as an \
                 identifier of nothing"
            );
        }
    }
    hidden.th.teardown().await;
}
