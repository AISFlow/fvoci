//! The activity of a card through the API (`getCardActivity`): every member of its
//! board reads it, the newest first and in pages; an agent's change names its token;
//! nobody else is told it is there.

use fvoci_model::Card;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::api_tokens::agent_writing;
use crate::cards::{board_with_roles, cards_path};
use crate::common::{Client, Setup, TEST_WS};

fn activity_path(card_id: &str) -> String {
    format!("{TEST_WS}/cards/{card_id}/activity")
}

/// Each line as its kind, its property and what it became.
async fn lines(client: &Client, card_id: &str, query: &str) -> Vec<(String, Value, Value)> {
    let lines: Vec<Value> = client
        .get(&format!("{}{query}", activity_path(card_id)))
        .await
        .ok_json();
    lines
        .iter()
        .map(|line| {
            (
                line["kind"].as_str().expect("a kind").to_owned(),
                line["propertyId"].clone(),
                line["after"].clone(),
            )
        })
        .collect()
}

#[tokio::test]
async fn every_member_of_the_board_reads_what_was_done_to_a_card() {
    let th = Setup::native().start().await.init_basic().await;
    let roles = board_with_roles(&th).await;
    let card: Card = roles
        .editor
        .post(&cards_path(&roles.board.id), &json!({"title": "first"}))
        .await
        .created_json();
    roles
        .editor
        .patch(
            &format!("{TEST_WS}/cards/{}", card.id),
            &json!({"title": "second"}),
        )
        .await
        .check_ok();
    let agent = agent_writing(&th, &th.client).await;
    agent
        .patch(
            &format!("{TEST_WS}/cards/{}", card.id),
            &json!({"title": "third"}),
        )
        .await
        .check_ok();

    let all: Vec<Value> = roles.viewer.get(&activity_path(&card.id)).await.ok_json();
    let editor_id = roles.editor.me().await.id;
    let owner_id = th.client.me().await.id;
    assert_eq!(all.len(), 3, "{all:?}");
    assert_eq!(
        (&all[0]["kind"], &all[0]["after"], &all[0]["actorId"]),
        (&json!("title"), &json!("third"), &json!(owner_id))
    );
    assert_eq!(all[0]["actorTokenName"], "agent", "{all:?}");
    assert_eq!(
        (&all[1]["before"], &all[1]["after"], &all[1]["actorId"]),
        (&json!("first"), &json!("second"), &json!(editor_id))
    );
    assert_eq!(all[1]["actorTokenName"], Value::Null);
    assert_eq!(all[2]["kind"], "created");
    for member in [&roles.commenter, &th.client] {
        assert_eq!(lines(member, &card.id, "").await.len(), 3);
    }
    assert_eq!(
        lines(&roles.viewer, &card.id, "?page=1&perPage=2").await,
        [("created".to_owned(), Value::Null, Value::Null)],
        "the second page of two"
    );
    roles
        .viewer
        .get(&format!("{}?perPage=0", activity_path(&card.id)))
        .await
        .check_status(StatusCode::BAD_REQUEST);

    // Nobody else is told there is such a card, nor of one in the trash.
    th.client2
        .get(&activity_path(&card.id))
        .await
        .check_status(StatusCode::NOT_FOUND);
    roles
        .editor
        .post(
            &format!("{}/batch-delete", cards_path(&roles.board.id)),
            &json!({"cardIds": [card.id]}),
        )
        .await
        .check_no_content();
    roles
        .viewer
        .get(&activity_path(&card.id))
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.teardown().await;
}
