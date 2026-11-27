//! The lists an agent finds its way by: the containers of a workspace by kind,
//! visibility, title and membership, the files of a board and of a card, and the cards
//! of a board chosen and ordered by the list's query, whole or in short.

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::api_tokens::agent_writing;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::files::{upload_in, uploaded_id};

async fn made(client: &Client, path: &str, title: &str, visibility: &str) -> String {
    let made: Value = client
        .post(
            &format!("{TEST_WS}/{path}"),
            &json!({"title": title, "visibility": visibility}),
        )
        .await
        .created_json();
    made["id"].as_str().expect("an id").to_owned()
}

/// The identifiers of the items of a list, in its order.
fn ids(list: &Value) -> Vec<&str> {
    list.as_array()
        .expect("a list")
        .iter()
        .map(|item| item["id"].as_str().expect("an id"))
        .collect()
}

/// Which of `wanted` the list holds, in the list's order.
fn among<'w>(list: &Value, wanted: &[&'w str]) -> Vec<&'w str> {
    let listed = ids(list);
    let mut found: Vec<&str> = wanted
        .iter()
        .copied()
        .filter(|id| listed.contains(id))
        .collect();
    found.sort_by_key(|id| listed.iter().position(|listed| listed == id));
    found
}

#[tokio::test]
async fn an_agent_lists_the_containers_it_reads_by_kind_visibility_title_and_membership() {
    let th = Setup::native().start().await.init_basic().await;
    // The first user's boards: one open, which the second user is no member of, and one
    // private, which they do not read.
    let unjoined = made(&th.client, "boards", "Roadmap of the lab", "open").await;
    let unread = made(&th.client, "boards", "Roadmap kept apart", "private").await;
    let joined = made(&th.client2, "boards", "Reading list", "open").await;
    let wiki = made(&th.client2, "wikis", "Lab roadmap notes", "private").await;
    let agent = agent_writing(&th, &th.client2).await;
    let all = [
        unjoined.as_str(),
        unread.as_str(),
        joined.as_str(),
        wiki.as_str(),
    ];
    let list = |query: &str| {
        let agent = &agent;
        let path = format!("{TEST_WS}/containers{query}");
        async move { agent.get(&path).await.ok_json::<Value>() }
    };

    assert_eq!(among(&list("").await, &all), [&unjoined, &joined, &wiki]);
    assert_eq!(among(&list("?kind=wiki").await, &all), [&wiki]);
    assert_eq!(
        among(&list("?kind=board&visibility=open").await, &all),
        [&unjoined, &joined]
    );
    assert_eq!(among(&list("?visibility=private").await, &all), [&wiki]);
    assert_eq!(among(&list("?q=ROADMAP").await, &all), [&unjoined, &wiki]);
    assert_eq!(
        among(&list("?joinedOnly=true").await, &all),
        [&joined, &wiki]
    );
    assert_eq!(
        among(&list("?joinedOnly=true&kind=board&q=reading").await, &all),
        [&joined]
    );
    // The boards' own list is grouped with them now and lists its boards alone.
    let boards: Value = agent.get(&format!("{TEST_WS}/boards")).await.ok_json();
    assert_eq!(among(&boards, &all), [&unjoined, &joined]);

    for refused in ["?kind=folder", "?visibility=hidden", "?sort=title"] {
        agent
            .get(&format!("{TEST_WS}/containers{refused}"))
            .await
            .check_status(StatusCode::BAD_REQUEST);
    }

    th.teardown().await;
}

#[tokio::test]
async fn an_agent_lists_the_files_of_a_board_and_those_a_card_shows() {
    let th = Setup::native().start().await.init_basic().await;
    let board = made(&th.client, "boards", "with files", "open").await;
    let upload = |name: &'static str| {
        let (client, board) = (&th.client, &board);
        async move { uploaded_id(&upload_in(client, TEST_WORKSPACE_ID, board, name, b"bytes").await) }
    };
    let shown = upload("moon.png").await;
    let attached = upload("report.pdf").await;
    let loose = upload("notes.txt").await;
    let image = json!({"type": "image", "attrs": {"fileId": shown}});
    let card: Value = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{board}/cards"),
            &json!({"title": "has files", "body": {"type": "doc", "content": [image]}}),
        )
        .await
        .created_json();
    let card_id = card["id"].as_str().expect("an id");
    let card_files = format!("{TEST_WS}/boards/{board}/cards/{card_id}/files");
    th.client
        .post(
            &format!("{TEST_WS}/boards/{board}/cards/{card_id}/attachments"),
            &json!({"fileId": attached}),
        )
        .await
        .check_created();
    let agent = agent_writing(&th, &th.client).await;

    let files: Value = agent
        .get(&format!("{TEST_WS}/containers/{board}/files"))
        .await
        .ok_json();
    let named: Vec<(&str, &str)> = files
        .as_array()
        .expect("a list")
        .iter()
        .map(|file| {
            (
                file["fileId"].as_str().expect("an id"),
                file["name"].as_str().expect("a name"),
            )
        })
        .collect();
    assert_eq!(
        named,
        [
            (shown.as_str(), "moon.png"),
            (attached.as_str(), "report.pdf"),
            (loose.as_str(), "notes.txt"),
        ]
    );
    assert_eq!(files[0]["size"], 5, "{files}");
    assert_eq!(files[0]["extension"], ".png", "{files}");

    let of_the_card: Value = agent.get(&card_files).await.ok_json();
    let of_the_card: Vec<&str> = of_the_card
        .as_array()
        .expect("a list")
        .iter()
        .map(|file| file["fileId"].as_str().expect("an id"))
        .collect();
    assert_eq!(of_the_card, [shown.as_str(), attached.as_str()]);

    let elsewhere = made(&th.client, "boards", "without the card", "open").await;
    agent
        .get(&format!(
            "{TEST_WS}/boards/{elsewhere}/cards/{card_id}/files"
        ))
        .await
        .check_status(StatusCode::NOT_FOUND);
    let private = made(&th.client, "boards", "not theirs", "private").await;
    th.client2
        .get(&format!("{TEST_WS}/containers/{private}/files"))
        .await
        .check_status(StatusCode::NOT_FOUND);

    th.teardown().await;
}

/// A board with an owner and a stage for its cards, and three cards: the first one's
/// own, changed last, a card below it, and the second user's.
struct Work {
    cards: String,
    alpha: String,
    beta: String,
    gamma: String,
    me: String,
    other: String,
    /// When the first card was changed.
    since: i64,
}

async fn three_cards(th: &TestServer) -> Work {
    let board = made(&th.client, "boards", "work", "open").await;
    th.client
        .patch(
            &format!("{TEST_WS}/containers/{board}"),
            &json!({"updatedCardProperties": [
                {"id": "owner", "name": "Owner", "type": "person"},
                {"id": "stage", "name": "Stage", "type": "select", "options": [
                    {"id": "doing", "label": "Doing"}, {"id": "done", "label": "Done"},
                ]},
            ]}),
        )
        .await
        .check_ok();
    let me = th.client.me().await.id;
    let other = th.client2.me().await.id;
    let cards = format!("{TEST_WS}/boards/{board}/cards");
    let card = |fields: Value| {
        let (client, cards) = (&th.client, &cards);
        async move {
            let made: Value = client.post(cards, &fields).await.created_json();
            made["id"].as_str().expect("an id").to_owned()
        }
    };
    let alpha =
        card(json!({"title": "Alpha plan", "properties": {"owner": me, "stage": "doing"}})).await;
    let beta = card(json!({"title": "Beta step", "parentId": alpha})).await;
    let gamma =
        card(json!({"title": "Gamma", "properties": {"owner": other, "stage": "done"}})).await;
    let changed: Value = th
        .client
        .patch(
            &format!("{TEST_WS}/cards/{alpha}"),
            &json!({"title": "Alpha plan, again"}),
        )
        .await
        .ok_json();
    Work {
        cards,
        alpha,
        beta,
        gamma,
        me,
        other,
        since: changed["updatedAt"].as_i64().expect("a time"),
    }
}

#[tokio::test]
async fn an_agent_asks_for_the_cards_it_wants_in_the_order_it_wants() {
    let th = Setup::native().start().await.init_basic().await;
    let Work {
        cards,
        alpha,
        beta,
        gamma,
        me,
        other,
        since,
    } = three_cards(&th).await;
    let agent = agent_writing(&th, &th.client).await;
    let chosen = |query: String| {
        let (agent, cards) = (&agent, &cards);
        async move {
            let list: Value = agent.get(&format!("{cards}?{query}")).await.ok_json();
            ids(&list)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        }
    };
    let none: [&str; 0] = [];

    assert_eq!(chosen("q=ALPHA".into()).await, [alpha.as_str()]);
    assert_eq!(chosen("q=%232".into()).await, [beta.as_str()]);
    assert_eq!(
        chosen("parentId=root".into()).await,
        [alpha.as_str(), gamma.as_str()]
    );
    assert_eq!(chosen(format!("parentId={alpha}")).await, [beta.as_str()]);
    assert_eq!(chosen("assigneeId=me".into()).await, [alpha.as_str()]);
    assert_eq!(
        chosen(format!("assigneeId={other}")).await,
        [gamma.as_str()]
    );
    assert_eq!(
        chosen("propertyId=stage&propertyValue=done".into()).await,
        [gamma.as_str()]
    );
    assert_eq!(
        chosen("propertyId=owner&propertyValue=nobody".into()).await,
        none
    );
    assert_eq!(chosen("sort=updated_desc".into()).await[0], alpha);
    assert_eq!(
        chosen("sort=number_asc".into()).await,
        [alpha.as_str(), beta.as_str(), gamma.as_str()]
    );
    assert_eq!(
        chosen("sort=number_asc&perPage=1&page=1".into()).await,
        [beta.as_str()]
    );
    assert!(
        chosen(format!("updatedSince={since}"))
            .await
            .contains(&alpha)
    );
    assert_eq!(chosen(format!("updatedSince={}", since + 1)).await, none);

    let short: Value = agent
        .get(&format!("{cards}?compact=true&parentId=root&assigneeId=me"))
        .await
        .ok_json();
    assert_eq!(
        short,
        json!([{
            "id": alpha, "number": 1, "parentId": "", "title": "Alpha plan, again",
            "properties": {"owner": me, "stage": "doing"}, "hasBody": false,
            "updatedAt": since,
        }])
    );

    th.teardown().await;
}

#[tokio::test]
async fn a_list_of_cards_refuses_what_it_cannot_ask_for() {
    let th = Setup::native().start().await.init_basic().await;
    let board = made(&th.client, "boards", "work", "open").await;
    let agent = agent_writing(&th, &th.client).await;
    for (refused, at) in [
        ("sort=title", "sort"),
        ("propertyId=stage", "propertyValue"),
        ("propertyValue=done", "propertyId"),
        ("updatedSince=yesterday", "updatedSince"),
    ] {
        let response = agent
            .get(&format!("{TEST_WS}/boards/{board}/cards?{refused}"))
            .await;
        response.check_status(StatusCode::BAD_REQUEST);
        assert!(
            response.text().contains(at),
            "{refused}: {}",
            response.text()
        );
    }

    th.teardown().await;
}
