//! A token given the account's own space reaches no diary of it: not by the routes of
//! diaries, and not by those of every kind of container, its files, uploads, search,
//! categories, trash and archives, which take a token or are a person's alone.

use fvoci_model::IssuedApiToken;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{ApiResponse, Client, PASSWORD, TestServer, UNKNOWN_ID};
use crate::diaries::{a_diary, in_a_space, last_of, text_of, write_body, written};
use crate::files::{upload_in, uploaded_id};

/// What the diary is known by: a word of its entry.
pub(crate) const WORD: &str = "보름달";

#[track_caller]
fn check_not_there(response: &ApiResponse) {
    assert_eq!(
        response.status,
        StatusCode::NOT_FOUND,
        "{} {}",
        response.path,
        response.text()
    );
}

#[track_caller]
fn check_a_persons(response: &ApiResponse) {
    assert_eq!(
        response.status,
        StatusCode::FORBIDDEN,
        "{} {}",
        response.path,
        response.text()
    );
    assert_eq!(
        response.error_reason(),
        "token_forbidden_endpoint",
        "{}",
        response.path
    );
}

/// A space with a diary, an entry that says [`WORD`] and a file, a board beside it,
/// and a token that writes and was given the space.
pub(crate) struct Space {
    pub(crate) ws: String,
    pub(crate) own: String,
    pub(crate) diary: String,
    pub(crate) diary_id: String,
    pub(crate) entry: String,
    pub(crate) file_id: String,
    pub(crate) board_id: String,
    pub(crate) agent: Client,
    pub(crate) th: TestServer,
}

pub(crate) async fn a_space_with_a_diary() -> Space {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let own = ws.trim_start_matches("/w/").to_owned();
    let diary = a_diary(client, &ws).await;
    let diary_id = last_of(&diary).to_owned();
    let made = written(client, &diary, &json!({"date": "2024-02-29"})).await;
    let entry = format!("{diary}/entries/{}", text_of(&made, "id"));
    write_body(client, &entry, &made, &format!("{WORD}을 봤다")).await;
    let file_id = uploaded_id(&upload_in(client, &own, &diary_id, "moon.png", b"moon").await);
    let board: Value = client
        .post(
            &format!("{ws}/boards"),
            &json!({"title": "plain", "visibility": "private"}),
        )
        .await
        .created_json();
    let board_id = text_of(&board, "id");
    assert!(
        th.app
            .finish_background(std::time::Duration::from_secs(5))
            .await,
        "the background work ends"
    );
    let issued: IssuedApiToken = client
        .post(
            "/users/me/api-tokens",
            &json!({
                "name": "agent", "scope": "write",
                "allowPersonalSpace": true, "password": PASSWORD,
            }),
        )
        .await
        .created_json();
    let agent = th.client_with_token(&issued.token);

    Space {
        ws,
        own,
        diary,
        diary_id,
        entry,
        file_id,
        board_id,
        agent,
        th,
    }
}

#[tokio::test]
async fn a_token_given_the_own_space_finds_no_diary_by_the_routes_it_takes() {
    let Space {
        th,
        ws,
        own,
        diary_id,
        file_id,
        board_id,
        agent,
        ..
    } = a_space_with_a_diary().await;
    let client = &th.client;
    // The space is the token's: what is not a diary in it is there.
    agent
        .get(&format!("{ws}/containers/{board_id}"))
        .await
        .check_ok();
    // The session finds the diary as the token does not.
    let found: Value = client.get(&format!("{ws}/search?q={WORD}")).await.ok_json();
    assert_eq!(found.as_array().map(Vec::len), Some(1), "{found}");
    let diaries: Value = client
        .get(&format!("{ws}/containers?kind=diary"))
        .await
        .ok_json();
    assert!(diaries.to_string().contains(&diary_id), "{diaries}");

    let container = format!("{ws}/containers/{diary_id}");
    let file = format!("{container}/files/{file_id}");
    for path in [
        container.clone(),
        format!("{container}/my-permissions"),
        file.clone(),
        format!("{file}/info"),
        format!("{file}/versions"),
        format!("{file}/versions/1"),
        format!("{container}/files"),
    ] {
        check_not_there(&agent.get(&path).await);
    }
    check_not_there(&upload_in(&agent, &own, &diary_id, "more.png", b"more").await);
    check_not_there(
        &agent
            .post(
                &format!("{container}/uploads"),
                &json!({"name": "big.bin", "size": 10}),
            )
            .await,
    );
    for search in [
        format!("{ws}/search?q={WORD}"),
        format!("{ws}/search?q=2024"),
        format!("{ws}/boards/search?q="),
        format!("{ws}/containers"),
        format!("{ws}/containers?kind=diary"),
    ] {
        let found: Value = agent.get(&search).await.ok_json();
        let said = found.to_string();
        assert!(!said.contains(&diary_id), "{search}: {said}");
        assert!(!said.contains(WORD), "{search}: {said}");
    }

    th.teardown().await;
}

#[tokio::test]
async fn a_token_given_the_own_space_is_refused_by_the_routes_of_a_person_and_of_diaries() {
    let Space {
        th,
        ws,
        own,
        diary,
        diary_id,
        entry,
        file_id,
        agent,
        ..
    } = a_space_with_a_diary().await;
    let container = format!("{ws}/containers/{diary_id}");
    let file = format!("{container}/files/{file_id}");
    let category = UNKNOWN_ID;
    let refused = [
        agent.get(&format!("{ws}/diaries")).await,
        agent.get(&format!("{diary}/days")).await,
        agent.get(&entry).await,
        agent.get(&format!("{entry}/body-versions")).await,
        agent
            .post(&format!("{entry}/body-versions"), &json!({}))
            .await,
        agent.get(&format!("{entry}/body-versions/1")).await,
        agent
            .post(&format!("{entry}/body-versions/1/restore"), &json!({}))
            .await,
        agent.get(&format!("{container}/archive/export")).await,
        agent.get(&format!("{container}/markdown/export")).await,
        agent.get(&format!("{ws}/archive/export")).await,
        agent
            .post(&format!("{ws}/archive/import"), &json!({}))
            .await,
        agent
            .post(
                &format!("{ws}/archive/uploads"),
                &json!({"name": "a.zip", "size": 10}),
            )
            .await,
        agent.get(&format!("{ws}/categories")).await,
        agent
            .post(&format!("{ws}/categories"), &json!({"name": "c"}))
            .await,
        agent
            .put(
                &format!("{ws}/categories/{category}/containers/{diary_id}/hide"),
                &json!({}),
            )
            .await,
        agent.get(&format!("{ws}/trash/containers")).await,
        agent
            .get(&format!("{ws}/boards/{diary_id}/trash/cards"))
            .await,
        agent.get(&format!("{file}/lease")).await,
        agent.put(&format!("{file}/lease"), &json!({})).await,
        agent.patch(&container, &json!({"title": "read"})).await,
        agent.delete(&container).await,
        agent
            .post(&format!("{container}/duplicate"), &json!({}))
            .await,
        agent
            .post(&format!("{container}/restore"), &json!({}))
            .await,
        agent
            .post(
                &format!("{container}/copy-to-workspace"),
                &json!({"targetWorkspaceId": own}),
            )
            .await,
        agent.post(&format!("{file}/versions"), &json!({})).await,
        agent
            .post(&format!("{file}/versions/1/restore"), &json!({}))
            .await,
    ];
    for response in &refused {
        check_a_persons(response);
    }

    th.teardown().await;
}

/// The routes of a board's views, attachments and card batches find no diary and no
/// entry of one, through the diary or through a board beside it: those a token takes
/// answer as for what is not there, and those of a person alone refuse the token
/// before they look. The entry stays as it was.
#[tokio::test]
async fn a_token_given_the_own_space_finds_no_diary_through_the_routes_of_a_board() {
    let Space {
        th,
        ws,
        diary_id,
        entry,
        board_id,
        agent,
        ..
    } = a_space_with_a_diary().await;
    let entry_id = last_of(&entry).to_owned();
    let before: Value = th.client.get(&entry).await.ok_json();
    for board in [&diary_id, &board_id] {
        let boards = format!("{ws}/boards/{board}");
        let batch = json!({"cardIds": [entry_id], "updatedProperties": {}});
        let mut taken = vec![
            agent.get(&format!("{boards}/views/{entry_id}")).await,
            agent
                .get(&format!("{boards}/cards/{entry_id}/attachments"))
                .await,
            agent
                .post(&format!("{boards}/cards/batch-patch"), &batch)
                .await,
        ];
        if board == &diary_id {
            taken.push(agent.get(&format!("{boards}/views")).await);
        }
        for response in &taken {
            check_not_there(response);
        }
        let attachment = format!("{boards}/cards/{entry_id}/attachments/{UNKNOWN_ID}");
        let a_persons = [
            agent
                .patch(
                    &format!("{boards}/views/{entry_id}"),
                    &json!({"title": "read"}),
                )
                .await,
            agent.delete(&format!("{boards}/views/{entry_id}")).await,
            agent
                .post(&format!("{boards}/views/{entry_id}/restore"), &json!({}))
                .await,
            agent.delete(&attachment).await,
            agent
                .post(&format!("{attachment}/restore"), &json!({}))
                .await,
        ];
        for response in &a_persons {
            check_a_persons(response);
        }
    }
    let after: Value = th.client.get(&entry).await.ok_json();
    assert_eq!(after, before, "the entry stays as it was");

    th.teardown().await;
}
