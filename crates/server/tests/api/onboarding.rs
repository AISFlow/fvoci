//! A new account's first board: a copy of the built-in welcome board.

use fvoci_model::{Block, BlockType, Container, ContainerVisibility, new_id};
use serde_json::{Value, json};

use crate::common::ROOT_WS;
use crate::common::TEST_WS;
use crate::common::{Client, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TestServer};

/// The title of the built-in welcome board, in English and in Korean.
const WELCOME_TITLE: &str = "Getting started with fvoci";
const WELCOME_TITLE_KO: &str = "fvoci 시작하기";

/// Makes an open template in the global workspace, as any signed-in user can, that looks as
/// much like the built-in welcome board as a client can make it: the same title and the
/// marker property. It holds one card, titled `hijacked`. Returns the template.
async fn impostor_template(client: &Client) -> Container {
    let board: Container = client
        .post(
            &format!("{ROOT_WS}/boards"),
            &json!({
                "workspaceId": ROOT_WORKSPACE_ID, "visibility": "open", "title": WELCOME_TITLE, "isTemplate": true,
                "properties": {"onboarding": true}, "createdBy": "system",
            }),
        )
        .await
        .created_json();
    let card = Block {
        id: new_id(),
        container_id: board.id.clone(),
        parent_id: String::new(),
        block_type: BlockType::Card,
        title: "hijacked".to_owned(),
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    crate::blocks::insert_in(client, ROOT_WORKSPACE_ID, &board.id, &[card]).await;
    board
}

/// The titles of the cards of a board.
async fn card_titles(ts: &TestServer, client: &Client, board_id: &str) -> Vec<String> {
    let _ = ts;
    let blocks: Vec<Block> = client
        .get(&format!("{ROOT_WS}/boards/{board_id}/blocks?type=card"))
        .await
        .ok_json();
    blocks.into_iter().map(|block| block.title).collect()
}

/// The built-in template that carries the onboarding mark, in the language the
/// templates are listed in for the client's user: English, here.
async fn marked_template(client: &Client) -> Container {
    let templates: Vec<Container> = client.get(&format!("{ROOT_WS}/templates")).await.ok_json();
    let marked: Vec<Container> = templates
        .into_iter()
        .filter(|board| {
            board.is_builtin && board.properties.get("onboarding") == Some(&json!(true))
        })
        .collect();
    assert_eq!(marked.len(), 1, "one built-in template is marked");
    marked.into_iter().next().expect("checked just above")
}

/// A first board is never copied from a template that a user made, whatever its title
/// and properties say: taking the first open template with the welcome board's title
/// would hand a user's template to every new user. What is copied is the template the
/// system made and marked, and nothing else: here there is none, so the built-in
/// templates are stored and their welcome board is copied.
#[tokio::test]
async fn onboarding_without_the_built_in_templates_does_not_copy_a_user_template() {
    let ts = Setup::native().start().await.init_basic().await;
    let impostor = impostor_template(&ts.client).await;
    assert_ne!(
        impostor.created_by, "system",
        "a client does not choose who created its board"
    );

    let value = ts
        .client2
        .post_empty(&format!("{ROOT_WS}/onboard"))
        .await
        .ok_json::<Value>();
    let board_id = value["boardId"].as_str().expect("a board id");
    let titles = card_titles(&ts, &ts.client2, board_id).await;
    assert!(!titles.is_empty(), "the welcome board has cards");
    assert!(
        !titles.iter().any(|title| title == "hijacked"),
        "not the user's template: {titles:?}"
    );
    let built_in = marked_template(&ts.client2).await;
    let mut expected = card_titles(&ts, &ts.client2, &built_in.id).await;
    let mut titles = titles;
    expected.sort();
    titles.sort();
    assert_eq!(titles, expected, "a copy of the built-in welcome board");
    ts.teardown().await;
}

/// With the built-in templates, the marked one is copied however many templates
/// users made to look like it: the same title, the mark as a property, `createdBy` in
/// the request.
#[tokio::test]
async fn onboarding_copies_the_built_in_board_whatever_users_made() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    for _ in 0..5 {
        impostor_template(&ts.client).await;
    }
    let built_in = marked_template(&ts.client2).await;
    let mut expected = card_titles(&ts, &ts.client2, &built_in.id).await;
    expected.sort();
    assert!(!expected.is_empty(), "the built-in board has cards");

    let value = ts
        .client2
        .post_empty(&format!("{ROOT_WS}/onboard"))
        .await
        .ok_json::<Value>();
    let board_id = value["boardId"].as_str().expect("a board id").to_owned();

    let mut copied = card_titles(&ts, &ts.client2, &board_id).await;
    copied.sort();
    assert_eq!(copied, expected, "the cards of the built-in board");
    let copy: Container = ts
        .client2
        .get(&format!("{ROOT_WS}/containers/{board_id}"))
        .await
        .ok_json();
    for mark in [
        "onboarding",
        "templateKey",
        "templateLanguage",
        "templateBaseDate",
    ] {
        assert_eq!(
            copy.properties.get(mark),
            None,
            "the copy is a user's board and does not carry the mark {mark}"
        );
    }
    assert_eq!(
        copy.visibility,
        ContainerVisibility::Private,
        "and it is private"
    );
    ts.teardown().await;
}

/// The mark, not the title, is what the welcome board is found by: the built-in board is
/// still found after its title changed.
#[tokio::test]
async fn onboarding_finds_the_built_in_board_by_its_mark_not_by_its_title() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    let built_in = marked_template(&ts.client).await;
    assert_eq!(built_in.title, WELCOME_TITLE, "as its file has it");
    let rename = fvoci_model::ContainerPatch {
        title: Some("Bienvenue".to_owned()),
        ..fvoci_model::ContainerPatch::default()
    };
    ts.system_store()
        .patch_container(&built_in.id, &rename, "")
        .await
        .expect("the built-in board is renamed");

    let value = ts
        .client
        .post_empty(&format!("{ROOT_WS}/onboard"))
        .await
        .ok_json::<Value>();
    let board_id = value["boardId"].as_str().expect("a board id").to_owned();

    let copy: Container = ts
        .client
        .get(&format!("{ROOT_WS}/containers/{board_id}"))
        .await
        .ok_json();
    assert_eq!(
        copy.title, "Bienvenue",
        "a copy of the renamed built-in board"
    );
    ts.teardown().await;
}

/// `onboard` answers a new board, which is private and the welcome board, and writes
/// nothing into the client's state.
#[tokio::test]
async fn onboard_copies_the_welcome_board_as_a_private_board() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;

    let response = ts.client.post_empty(&format!("{ROOT_WS}/onboard")).await;
    let value = response.ok_json::<Value>();
    assert_eq!(
        value,
        json!({"boardId": value["boardId"]}),
        "the board, and nothing else"
    );
    let board_id = value["boardId"].as_str().expect("a board id").to_owned();
    assert!(!board_id.is_empty(), "a board id is returned");

    let board = ts
        .client
        .get(&format!("{ROOT_WS}/containers/{board_id}"))
        .await
        .ok_json::<Container>();
    assert_eq!(
        board.visibility,
        ContainerVisibility::Private,
        "the new board is private"
    );
    assert_eq!(board.title, WELCOME_TITLE, "it is the welcome board");

    let user_id = ts.client.me().await.id;
    let client_state = ts
        .app
        .client_state(&user_id)
        .await
        .expect("the client state");
    assert!(
        client_state.is_empty(),
        "nothing is noted in the client's state: {client_state:?}"
    );
    ts.teardown().await;
}

/// The welcome board is made in the workspace of the path.
#[tokio::test]
async fn onboard_makes_the_board_in_the_workspace_of_the_path() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    let value = ts
        .client
        .post_empty(&format!("{TEST_WS}/onboard"))
        .await
        .ok_json::<Value>();
    let board_id = value["boardId"].as_str().expect("a board id");
    let board: Container = ts
        .client
        .get(&format!("{TEST_WS}/containers/{board_id}"))
        .await
        .ok_json();
    assert_eq!(board.workspace_id, TEST_WORKSPACE_ID, "the workspace");
    ts.teardown().await;
}

/// The title of the board that `onboard` makes for the client's user when the request
/// accepts `language` (says nothing when empty).
async fn welcome_title(client: &Client, language: &str) -> String {
    let mut request = client.request(reqwest::Method::POST, &format!("{ROOT_WS}/onboard"));
    if !language.is_empty() {
        request = request.header(reqwest::header::ACCEPT_LANGUAGE, language);
    }
    let value = Client::send(request).await.ok_json::<Value>();
    let board_id = value["boardId"].as_str().expect("a board id");
    let board: Container = client
        .get(&format!("{ROOT_WS}/containers/{board_id}"))
        .await
        .ok_json();
    board.title
}

/// The welcome board is written in the language of the user's settings; where the
/// settings leave the language to the device, or name one the board is not written in,
/// in the language the request accepts first; failing both, in English.
#[tokio::test]
async fn the_welcome_board_is_in_the_language_of_the_settings_or_else_of_the_request() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    let client = &ts.client;
    let set = async |language: Value| {
        client
            .patch("/users/me/settings", &json!({"language": language}))
            .await
            .check_ok();
    };

    // The settings leave it to the device: the request says.
    assert_eq!(
        welcome_title(client, "").await,
        WELCOME_TITLE,
        "nothing said"
    );
    assert_eq!(welcome_title(client, "ko").await, WELCOME_TITLE_KO, "ko");
    assert_eq!(
        welcome_title(client, "ko-KR").await,
        WELCOME_TITLE_KO,
        "ko-KR"
    );
    assert_eq!(welcome_title(client, "en-GB").await, WELCOME_TITLE, "en-GB");
    assert_eq!(welcome_title(client, "ja").await, WELCOME_TITLE, "ja");

    // The settings say: the request is not asked.
    set(json!("ko")).await;
    assert_eq!(
        welcome_title(client, "").await,
        WELCOME_TITLE_KO,
        "set to ko"
    );
    assert_eq!(
        welcome_title(client, "en").await,
        WELCOME_TITLE_KO,
        "set to ko, where English is accepted"
    );
    set(json!("en-US")).await;
    assert_eq!(
        welcome_title(client, "ko").await,
        WELCOME_TITLE,
        "set to en-US, where Korean is accepted"
    );

    // A language the board is not written in: the request says again.
    set(json!("ja")).await;
    assert_eq!(
        welcome_title(client, "ko").await,
        WELCOME_TITLE_KO,
        "set to ja, where Korean is accepted"
    );
    assert_eq!(
        welcome_title(client, "ja").await,
        WELCOME_TITLE,
        "ja and ja"
    );
    ts.teardown().await;
}

/// `POST /w/0/onboard` without a session is refused.
#[tokio::test]
async fn onboard_without_a_session_is_refused() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    ts.anonymous()
        .post_empty(&format!("{ROOT_WS}/onboard"))
        .await
        .check_unauthorized();
    ts.teardown().await;
}

/// Without a built-in welcome board, `onboard` stores the built-in templates again and
/// copies the board they hold, instead of failing.
#[tokio::test]
async fn onboard_stores_the_templates_again_when_the_welcome_board_is_gone() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    ts.app
        .store()
        .remove_builtin_templates()
        .await
        .expect("the built-in templates are removed");

    let value = ts
        .client
        .post_empty(&format!("{ROOT_WS}/onboard"))
        .await
        .ok_json::<Value>();
    let board_id = value["boardId"].as_str().expect("a board id");
    let board: Container = ts
        .client
        .get(&format!("{ROOT_WS}/containers/{board_id}"))
        .await
        .ok_json();
    assert_eq!(board.title, WELCOME_TITLE, "the welcome board is copied");
    marked_template(&ts.client).await;
    ts.teardown().await;
}

/// Onboarding requests that find the welcome board gone at the same time store the
/// built-in templates once between them: the decision and the storing are one
/// transaction.
#[tokio::test]
async fn concurrent_onboarding_stores_the_templates_once() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    ts.app
        .store()
        .remove_builtin_templates()
        .await
        .expect("the built-in templates are removed");

    let path = format!("{ROOT_WS}/onboard");
    let answers = futures_util::future::join_all(
        [&ts.client, &ts.client2, &ts.client, &ts.client2].map(|client| client.post_empty(&path)),
    )
    .await;
    for answer in answers {
        answer.ok_json::<Value>();
    }
    let templates = ts
        .app
        .store()
        .get_builtin_templates()
        .await
        .expect("the built-in templates are read");
    assert_eq!(
        templates.len(),
        18,
        "not stored twice over, and every onboarding found its welcome board"
    );
    ts.teardown().await;
}
