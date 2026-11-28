//! What an agent does through a personal API token is told as its doing: to whom it
//! names, and to the token's owner as well where the token asks for that
//! (`notifySelf`, on unless the owner turned it off), but never in the owner's own
//! space, where nobody is told anything. A person is still told nothing of their own
//! doing.

use fvoci_model::{Card, ContainerVisibility, IssuedApiToken, Notification};
use serde_json::{Value, json};

use crate::api_tokens::agent_writing;
use crate::common::{Client, PASSWORD, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::notifications::{
    OWNER, board_with_owner, card_with, comment_on, give, kinds, mentioning, plain, told,
};

/// A token of `client`'s user that writes in the test workspace and tells its owner
/// nothing of what it does.
async fn quiet_agent(th: &TestServer, client: &Client) -> Client {
    let issued: IssuedApiToken = client
        .post(
            "/users/me/api-tokens",
            &json!({
                "name": "quiet", "scope": "write", "workspaceIds": [TEST_WORKSPACE_ID],
                "notifySelf": false,
            }),
        )
        .await
        .created_json();
    assert!(!issued.api_token.notify_self, "{issued:?}");
    th.client_with_token(&issued.token)
}

/// Writes a comment of `body` on `card` as `agent`, through the route a token takes.
async fn agent_comments(agent: &Client, card: &Card, body: &Value) {
    agent
        .post(
            &format!(
                "{TEST_WS}/boards/{}/cards/{}/comments",
                card.board_id, card.id
            ),
            &json!({"body": body}),
        )
        .await
        .check_created();
}

/// The kinds told, each with its doer and the token named, sorted.
fn told_as(told: &[Notification]) -> Vec<(&'static str, String, Option<String>)> {
    let mut told: Vec<_> = told
        .iter()
        .filter_map(|told| told.subject.as_ref())
        .map(|subject| {
            (
                subject.kind.as_str(),
                subject.actor_id.clone(),
                subject.agent_token_name.clone(),
            )
        })
        .collect();
    told.sort();
    told
}

/// An agent of A comments on a card A watches, mentions A and B in a comment on
/// another, gives A a third, and names A in Markdown it adds to a fourth's body. A is
/// told of the four as the agent's doing, by A through the token; B of the mention.
#[tokio::test]
async fn an_agent_tells_its_owner_of_its_comment_its_mention_and_the_card_it_gives() {
    let th = crate::common::Setup::native()
        .start()
        .await
        .init_basic()
        .await;
    let (a, b) = (th.client.me().await, th.client2.me().await);
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    let agent = agent_writing(&th, &th.client).await;
    let watched = card_with(&th.client, &board.id, &Value::Null).await;
    let named = card_with(&th.client, &board.id, &Value::Null).await;
    let given = card_with(&th.client, &board.id, &Value::Null).await;
    let written = card_with(&th.client, &board.id, &Value::Null).await;

    agent_comments(&agent, &watched, &plain("done")).await;
    agent_comments(&agent, &named, &mentioning(&[&a.id, &b.id])).await;
    agent
        .patch(
            &format!("{TEST_WS}/cards/{}", given.id),
            &json!({"updatedProperties": {OWNER: a.id}}),
        )
        .await
        .check_ok();
    agent
        .post(
            &format!("{TEST_WS}/cards/{}/body/markdown", written.id),
            &json!({"markdown": format!("for @[](fvoci-user://{})", a.id)}),
        )
        .await
        .check_ok();

    let agents = Some("agent".to_owned());
    let theirs = told(&th, &th.client).await;
    assert_eq!(
        told_as(&theirs),
        vec![
            ("assignment", a.id.clone(), agents.clone()),
            ("comment", a.id.clone(), agents.clone()),
            ("mention", a.id.clone(), agents.clone()),
            ("mention", a.id.clone(), agents.clone()),
        ],
        "{theirs:?}"
    );
    let bs = told(&th, &th.client2).await;
    assert_eq!(
        told_as(&bs),
        vec![("mention", a.id.clone(), agents)],
        "{bs:?}"
    );

    th.teardown().await;
}

/// A mentions themselves, and a token of A's that tells A nothing comments on a card
/// A watches and mentions A and B: A is told of none of it, B of the mention as the
/// token's doing.
#[tokio::test]
async fn nobody_is_told_of_their_own_doing_nor_of_a_token_that_tells_them_nothing() {
    let th = crate::common::Setup::native()
        .start()
        .await
        .init_basic()
        .await;
    let (a, b) = (th.client.me().await, th.client2.me().await);
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    let card = card_with(&th.client, &board.id, &Value::Null).await;
    comment_on(&th.client, &card, &mentioning(&[&a.id])).await;
    let quiet = quiet_agent(&th, &th.client).await;
    agent_comments(&quiet, &card, &plain("done")).await;
    agent_comments(&quiet, &card, &mentioning(&[&a.id, &b.id])).await;

    let theirs = told(&th, &th.client).await;
    assert_eq!(kinds(&theirs), (vec![], 0), "{theirs:?}");
    let bs = told(&th, &th.client2).await;
    assert_eq!(
        told_as(&bs),
        vec![("mention", a.id.clone(), Some("quiet".to_owned()))],
        "{bs:?}"
    );

    th.teardown().await;
}

/// In the owner's own space nobody is told anything, also of a token that tells them.
#[tokio::test]
async fn an_agent_tells_its_owner_nothing_of_their_own_space() {
    let (th, ws) = crate::diaries::in_a_space().await;
    let a = th.client.me().await;
    let board: fvoci_model::Container = th
        .client
        .post(
            &format!("{ws}/boards"),
            &json!({"title": "mine", "visibility": "private"}),
        )
        .await
        .created_json();
    let card: Card = th
        .client
        .post(
            &format!("{ws}/boards/{}/cards", board.id),
            &json!({"title": "The card"}),
        )
        .await
        .created_json();
    let issued: IssuedApiToken = th
        .client
        .post(
            "/users/me/api-tokens",
            &json!({
                "name": "agent", "scope": "write",
                "allowPersonalSpace": true, "password": PASSWORD,
            }),
        )
        .await
        .created_json();
    assert!(issued.api_token.notify_self, "{issued:?}");
    let agent = th.client_with_token(&issued.token);
    agent
        .post(
            &format!("{ws}/boards/{}/cards/{}/comments", board.id, card.id),
            &json!({"body": mentioning(&[&a.id])}),
        )
        .await
        .check_created();

    let theirs = told(&th, &th.client).await;
    assert_eq!(kinds(&theirs), (vec![], 0), "{theirs:?}");

    th.teardown().await;
}

/// An agent of A that gives cards by `batchPatchCards` tells whom it newly gives each to
/// as `patchCard` tells of one: B of the two cards of the batch and of the one given
/// alone, A of theirs (the token tells its owner), each as the agent's doing; nobody of a
/// card given to them before, nor of a batch that tells nobody (`disableNotify`).
#[tokio::test]
async fn an_agent_tells_of_the_cards_it_gives_in_a_batch_as_of_one_given_alone() {
    let th = crate::common::Setup::native()
        .start()
        .await
        .init_basic()
        .await;
    let (a, b) = (th.client.me().await, th.client2.me().await);
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &b.id, "editor").await;
    let agent = agent_writing(&th, &th.client).await;
    let mut cards = Vec::new();
    for _ in 0..5 {
        cards.push(card_with(&th.client, &board.id, &Value::Null).await);
    }
    let batch = format!("{TEST_WS}/boards/{}/cards/batch-patch", board.id);
    let give_in_batch = async |path: &str, given: &[&Card], to: &str| {
        let ids: Vec<&str> = given.iter().map(|card| card.id.as_str()).collect();
        agent
            .post(
                path,
                &json!({"cardIds": ids, "updatedProperties": {OWNER: to}}),
            )
            .await
            .check_ok();
    };

    give_in_batch(&batch, &[&cards[0], &cards[1]], &b.id).await;
    agent
        .patch(
            &format!("{TEST_WS}/cards/{}", cards[2].id),
            &json!({"updatedProperties": {OWNER: b.id}}),
        )
        .await
        .check_ok();
    give_in_batch(&batch, &[&cards[0], &cards[1], &cards[2]], &b.id).await;
    give_in_batch(&batch, &[&cards[3]], &a.id).await;
    give_in_batch(&format!("{batch}?disableNotify=true"), &[&cards[4]], &b.id).await;

    let agents = Some("agent".to_owned());
    let bs = told(&th, &th.client2).await;
    assert_eq!(
        told_as(&bs),
        vec![("assignment", a.id.clone(), agents.clone()); 3],
        "{bs:?}"
    );
    let theirs = told(&th, &th.client).await;
    assert_eq!(
        told_as(&theirs),
        vec![("assignment", a.id.clone(), agents)],
        "{theirs:?}"
    );

    th.teardown().await;
}
