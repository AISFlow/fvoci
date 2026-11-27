//! A shared board read by its link: `/shared/{boardId}` with the read token, and no
//! session.

use fvoci_model::{Container, PublicLink, Workspace, WorkspaceRole, new_id};
use fvoci_store::DbType;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::TEST_WS;
use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID, TestServer, UNKNOWN_ID};

/// A board of the first user with what names people: a person property, a card whose
/// value of it is the second user, whose body mentions the first, a comment of the
/// second user, and a file.
struct Fixture {
    ts: TestServer,
    board: Container,
    card_id: String,
    file_id: String,
    token: String,
    /// What names the two users: their identifiers, names and email addresses.
    people: Vec<String>,
}

async fn fixture() -> Fixture {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let me = ts.client.me().await;
    let other = ts.client2.me().await;
    let board: Container = ts
        .client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({
                "workspaceId": TEST_WORKSPACE_ID,
                "visibility": "open",
                "title": "Shared",
                "cardProperties": [{"id": "owner", "name": "Owner", "type": "person", "options": []}],
            }),
        )
        .await
        .created_json();
    let card: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({
                "title": "Ask around",
                "properties": {"owner": other.id},
                "body": {"type": "doc", "content": [{"type": "paragraph", "content": [
                    {"type": "text", "text": "ask "},
                    {"type": "mention", "attrs": {"userId": me.id}},
                ]}]},
            }),
        )
        .await
        .created_json();
    let card_id = card["id"].as_str().expect("an identifier").to_owned();
    ts.client2
        .post(
            &format!("{TEST_WS}/containers/{}/join", board.id),
            &json!({}),
        )
        .await
        .check_ok();
    ts.client2
        .post(
            &format!("{TEST_WS}/boards/{}/blocks", board.id),
            &json!([{"id": "comment", "containerId": board.id, "parentId": card_id, "type": "comment", "title": "fine by me", "createdAt": 1, "updatedAt": 1}]),
        )
        .await
        .check_created();
    let file_id = upload(&ts.client, &board.id).await;
    let sharing: PublicLink = ts
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/public-link", board.id),
            &json!({"enabled": true}),
        )
        .await
        .ok_json();
    let people = [me, other]
        .into_iter()
        .flat_map(|user| [user.id, user.username, user.email])
        .filter(|name| !name.is_empty())
        .collect();
    Fixture {
        ts,
        board,
        card_id,
        file_id,
        token: sharing.token,
        people,
    }
}

/// Uploads a file to the board and returns its identifier.
async fn upload(client: &Client, board_id: &str) -> String {
    let file = reqwest::multipart::Part::bytes(b"the shared file".to_vec()).file_name("a.txt");
    let uploaded: Value = Client::send(
        client
            .request(
                Method::POST,
                &format!("{TEST_WS}/containers/{board_id}/files"),
            )
            .multipart(reqwest::multipart::Form::new().part("file", file)),
    )
    .await
    .created_json();
    uploaded["fileId"].as_str().expect("a file").to_owned()
}

/// The paths under `/shared` that read the board, each with the token.
fn shared_paths(board_id: &str, file_id: &str, token: &str) -> Vec<String> {
    [
        String::new(),
        "/blocks".to_owned(),
        "/blocks?all=true".to_owned(),
        "/cards?preview=true".to_owned(),
        format!("/files/{file_id}"),
        format!("/files/{file_id}/info"),
    ]
    .into_iter()
    .map(|rest| {
        let joiner = if rest.contains('?') { '&' } else { '?' };
        format!("/shared/{board_id}{rest}{joiner}readToken={token}")
    })
    .collect()
}

#[track_caller]
fn check_names_nobody(response: &ApiResponse, people: &[String], path: &str) {
    response.check_ok();
    let text = response.text();
    for person in people {
        assert!(
            !text.contains(person.as_str()),
            "{path} names {person}: {text}"
        );
    }
}

/// The link alone reads the board, its blocks and cards and its file, and nothing of
/// the answers names a user: not without a session, and not for a member of the board
/// who is signed in either.
#[tokio::test]
async fn a_shared_board_is_read_by_its_token_alone_and_names_nobody() {
    let f = fixture().await;
    let paths = shared_paths(&f.board.id, &f.file_id, &f.token);
    for client in [f.ts.anonymous(), f.ts.client.clone()] {
        for path in &paths {
            check_names_nobody(&client.get(path).await, &f.people, path);
        }
    }
    let anonymous = f.ts.anonymous();
    let board: Value = anonymous.get(&paths[0]).await.ok_json();
    assert_eq!(
        (&board["id"], &board["title"], &board["workspaceId"]),
        (&json!(f.board.id), &json!("Shared"), &json!(""))
    );
    let blocks: Vec<Value> = anonymous.get(&paths[2]).await.ok_json();
    let comment = blocks
        .iter()
        .find(|block| block["type"] == "comment")
        .expect("the comment");
    assert_eq!(
        (&comment["title"], &comment["createdBy"]),
        (&json!("fine by me"), &json!(""))
    );
    let cards: Vec<Value> = anonymous.get(&paths[3]).await.ok_json();
    assert_eq!(cards.len(), 1, "the card");
    assert_eq!(cards[0]["id"], json!(f.card_id));
    assert_eq!(cards[0]["properties"]["owner"], json!(""), "who it names");
    let card: Vec<Value> = anonymous
        .get(&format!(
            "/shared/{}/blocks?blockId={}&readToken={}",
            f.board.id, f.card_id, f.token
        ))
        .await
        .ok_json();
    assert!(
        card[0]["body"].to_string().contains(r#""userId":"""#),
        "the mention is kept, naming nobody: {}",
        card[0]["body"]
    );
    let file = anonymous.get(&paths[4]).await;
    file.check_ok();
    assert_eq!(file.body, b"the shared file");
    f.ts.teardown().await;
}

/// A wrong token, a board that is not shared, and a board in the trash answer as a
/// board that is not there; so does a file of another board.
#[tokio::test]
async fn a_shared_board_is_not_there_without_its_token() {
    let f = fixture().await;
    let anonymous = f.ts.anonymous();
    let not_there = |path: String| {
        let anonymous = anonymous.clone();
        async move {
            let response = anonymous.get(&path).await;
            (response.status, response.error_message())
        }
    };
    let absent = not_there(format!("/shared/{UNKNOWN_ID}?readToken={}", f.token)).await;
    assert_eq!(absent.0, StatusCode::NOT_FOUND);
    for path in shared_paths(&f.board.id, &f.file_id, "wrong")
        .into_iter()
        .chain(shared_paths(&f.board.id, &f.file_id, ""))
        .chain([format!("/shared/{}", f.board.id)])
    {
        assert_eq!(not_there(path.clone()).await, absent, "{path}");
    }
    // Another board's file, with this board's token, is a file that is not there.
    let other: Container = f
        .ts
        .client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Other"}),
        )
        .await
        .created_json();
    let others_file = upload(&f.ts.client, &other.id).await;
    let file_path = |file_id: &str| {
        format!(
            "/shared/{}/files/{file_id}?readToken={}",
            f.board.id, f.token
        )
    };
    let missing = not_there(file_path("7abcdefabcdefabcdefabcdefab.txt")).await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
    assert_eq!(not_there(file_path(&others_file)).await, missing);

    f.ts.client
        .post(
            &format!("{TEST_WS}/containers/{}/public-link", f.board.id),
            &json!({"enabled": false}),
        )
        .await
        .check_ok();
    for path in shared_paths(&f.board.id, &f.file_id, &f.token) {
        assert_eq!(not_there(path.clone()).await, absent, "not shared: {path}");
    }
    f.ts.client
        .post(
            &format!("{TEST_WS}/containers/{}/public-link", f.board.id),
            &json!({"enabled": true}),
        )
        .await
        .check_ok();
    let token = f.ts.share(&f.board.id).await;
    f.ts.client
        .delete(&format!("{TEST_WS}/containers/{}", f.board.id))
        .await
        .check_no_content();
    for path in shared_paths(&f.board.id, &f.file_id, &token) {
        assert_eq!(
            not_there(path.clone()).await,
            absent,
            "in the trash: {path}"
        );
    }
    f.ts.teardown().await;
}

/// The routes of the board take no read token: without a session they are 401 whatever
/// the query says, and a user who is no member of the private board does not reach it
/// with one.
#[tokio::test]
async fn the_board_routes_take_no_read_token() {
    let f = fixture().await;
    f.ts.client
        .patch(
            &format!("{TEST_WS}/containers/{}", f.board.id),
            &json!({"visibility": "private"}),
        )
        .await
        .check_ok();
    let outsider = f.ts.add_user("outsider").await;
    for (under, rest) in [
        ("containers", String::new()),
        ("boards", "/blocks".to_owned()),
        ("boards", "/cards".to_owned()),
        ("containers", format!("/files/{}", f.file_id)),
        ("containers", format!("/files/{}/info", f.file_id)),
    ] {
        let path = format!(
            "{TEST_WS}/{under}/{}{rest}?readToken={}",
            f.board.id, f.token
        );
        f.ts.anonymous().get(&path).await.check_unauthorized();
        let response = outsider.get(&path).await;
        if under == "boards" {
            response.check_bad_request();
            assert_eq!(response.refused_at(), "parameter readToken", "{path}");
        } else {
            assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}");
        }
    }
    f.ts.teardown().await;
}

/// An address that has had many answers that its token opens nothing is refused for a
/// while, a good link from it too; the reads of a good link count nothing.
#[tokio::test]
async fn guessing_tokens_is_limited_by_address() {
    let f = fixture().await;
    let anonymous = f.ts.anonymous();
    let good = format!("/shared/{}?readToken={}", f.board.id, f.token);
    for _ in 0..40 {
        anonymous.get(&good).await.check_ok();
    }
    let max = fvoci_server::app::MAX_SHARED_READ_FAILURES;
    for attempt in 0..max {
        let response = anonymous
            .get(&format!("/shared/{}?readToken=guess{attempt}", f.board.id))
            .await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "attempt {attempt}");
    }
    let refused = anonymous.get(&good).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        !refused.header("Retry-After").is_empty(),
        "how long to wait"
    );
    f.ts.teardown().await;
}

/// The board is found in its own organization, whichever that is: a board of a second
/// organization is read by its link like any other.
#[tokio::test]
async fn a_board_of_a_second_organization_is_read_by_its_link() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    if fvoci_store::testing::db_type_of_the_tests() != DbType::Postgres {
        ts.teardown().await;
        return;
    }
    let workspace = Workspace {
        id: new_id(),
        ..Workspace::default()
    };
    let org = ts
        .app
        .store()
        .create_organization("Second", &workspace)
        .await
        .expect("a second organization");
    let theirs = ts.app.store().for_org(&org).expect("an identifier");
    let owner = ts.client2.me().await;
    theirs
        .add_workspace_member(&workspace.id, &owner.id, WorkspaceRole::Member)
        .await
        .expect("a member of the second organization");
    let mut board = Container {
        id: new_id(),
        workspace_id: workspace.id.clone(),
        title: "Elsewhere".to_owned(),
        ..Container::default()
    };
    theirs
        .insert_container_with_admin(&mut board, &owner.id)
        .await
        .expect("a board of the second organization");
    let token = ts
        .app
        .scoped(fvoci_store::Tenant::org(&org).expect("an identifier"))
        .set_public_link(&board.id, fvoci_model::PublicLinkChange::On, "")
        .await
        .expect("shared")
        .token;
    let read: Value = ts
        .anonymous()
        .get(&format!("/shared/{}?readToken={token}", board.id))
        .await
        .ok_json();
    assert_eq!(read["title"], json!("Elsewhere"));
    assert_eq!(read["createdBy"], json!(""));
    ts.teardown().await;
}
