//! A board made from a built-in template: somebody's own board, without the marks of
//! the template, its days moved to the day it is made on.

use std::collections::BTreeMap;

use fvoci_model::{Block, BlockType, Container, ContainersAndBlocks, DuplicateAs};
use serde_json::{Value, json};

use crate::common::{Client, ROOT_WORKSPACE_ID, ROOT_WS, Setup, TestServer};

/// 2026-12-31 20:00 UTC: the last evening of the year, and already the first morning
/// of the next in Seoul.
const NEW_YEARS_EVE: i64 = 1_798_747_200_000;

/// The marks of a built-in template among its board's properties.
const MARKS: [&str; 4] = [
    "onboarding",
    "templateKey",
    "templateLanguage",
    "templateBaseDate",
];

/// The built-in template of `key` as listed for the client's user.
async fn template(client: &Client, key: &str) -> Container {
    client
        .get(&format!("{ROOT_WS}/templates"))
        .await
        .ok_json::<Vec<Container>>()
        .into_iter()
        .find(|board| board.is_builtin && board.properties.get("templateKey") == Some(&json!(key)))
        .unwrap_or_else(|| panic!("the built-in template {key}"))
}

/// The date values of the cards of a board, by the title of the card: every value of a
/// card's that has a `from`.
async fn days_of(client: &Client, board_id: &str) -> BTreeMap<String, Vec<Value>> {
    let blocks: Vec<Block> = client
        .get(&format!("{ROOT_WS}/boards/{board_id}/blocks"))
        .await
        .ok_json();
    days_among(&blocks)
}

fn days_among(blocks: &[Block]) -> BTreeMap<String, Vec<Value>> {
    blocks
        .iter()
        .filter(|block| block.block_type == BlockType::Card)
        .map(|card| {
            let values = card.fields.get("properties").and_then(Value::as_object);
            let days = values
                .into_iter()
                .flat_map(|values| values.values())
                .filter(|value| value.get("from").is_some())
                .cloned()
                .collect();
            (card.title.clone(), days)
        })
        .collect()
}

/// A board of the client's user made from `template_id` at [`NEW_YEARS_EVE`].
async fn made_on_new_years_eve(
    ts: &TestServer,
    client: &Client,
    template_id: &str,
) -> ContainersAndBlocks {
    let user = client.me().await;
    let (copy, _) = ts
        .app
        .duplicate_container_at(
            template_id,
            &user.id,
            ROOT_WORKSPACE_ID,
            DuplicateAs::Copy,
            None,
            NEW_YEARS_EVE,
        )
        .await
        .expect("the template is copied");
    copy
}

/// The days of a built-in template are counted from the day it was written for, and a
/// board made from it has them counted from the day it is for whoever makes it: in the
/// time zone of their settings, or in UTC where the settings leave it to the device.
#[tokio::test]
async fn a_board_made_from_a_built_in_template_has_its_days_moved_to_the_makers_day() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    let schedule = template(&ts.client, "project-schedule").await;
    assert_eq!(
        schedule.properties.get("templateBaseDate"),
        Some(&json!("2026-12-01")),
        "the day the template was written for"
    );
    let written = days_of(&ts.client, &schedule.id).await;
    let first = "Agree on the scope";
    assert_eq!(
        written[first],
        [json!({"from": "2026-12-01", "to": "2026-12-03"})],
        "as the file has it"
    );

    // The settings leave the zone to the device: the day in UTC, thirty days on.
    let copy = made_on_new_years_eve(&ts, &ts.client, &schedule.id).await;
    let moved = days_among(&copy.blocks);
    assert_eq!(
        moved[first],
        [json!({"from": "2026-12-31", "to": "2027-01-02"})],
        "as it is answered"
    );
    assert_eq!(
        days_of(&ts.client, &copy.containers[0].id).await,
        moved,
        "and as it is stored"
    );
    assert_eq!(
        moved.keys().collect::<Vec<_>>(),
        written.keys().collect::<Vec<_>>(),
        "the same cards"
    );
    assert!(
        written.iter().all(|(title, days)| days != &moved[title]),
        "every card of the template has a day, and each is moved: {moved:?}"
    );

    // The settings name a zone where the next day has begun: thirty-one days on.
    ts.client
        .patch("/users/me/settings", &json!({"timeZone": "Asia/Seoul"}))
        .await
        .check_ok();
    let copy = made_on_new_years_eve(&ts, &ts.client, &schedule.id).await;
    assert_eq!(
        days_among(&copy.blocks)[first],
        [json!({"from": "2027-01-01", "to": "2027-01-03"})],
        "the day in Seoul"
    );
    // The other user's settings are their own.
    let copy = made_on_new_years_eve(&ts, &ts.client2, &schedule.id).await;
    assert_eq!(
        days_among(&copy.blocks)[first],
        [json!({"from": "2026-12-31", "to": "2027-01-02"})],
        "the day in UTC for the other user"
    );

    // The template itself stays as it was written.
    assert_eq!(days_of(&ts.client, &schedule.id).await, written);
    ts.teardown().await;
}

/// A board made from a built-in template is somebody's own: it carries none of the
/// marks that say a board is a built-in template. A copy of that board, which is no
/// template, keeps its days whenever it is made.
#[tokio::test]
async fn a_board_made_from_a_built_in_template_is_unmarked_and_its_copy_keeps_its_days() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    for key in ["welcome", "meeting-notes"] {
        let template = template(&ts.client, key).await;
        for mark in MARKS
            .iter()
            .filter(|mark| key == "welcome" || **mark != "onboarding")
        {
            assert!(
                template.properties.contains_key(*mark),
                "{key} is marked with {mark}"
            );
        }
        // By the route, on the day it is.
        let made: ContainersAndBlocks = ts
            .client
            .post_empty(&format!("{ROOT_WS}/containers/{}/duplicate", template.id))
            .await
            .created_json();
        let [board] = made.containers.as_slice() else {
            panic!("one board is made: {:?}", made.containers);
        };
        assert!(!board.is_template && !board.is_builtin, "{key}: a board");
        for mark in MARKS {
            assert_eq!(
                board.properties.get(mark),
                None,
                "{key}: answered without {mark}"
            );
        }
        let stored: Container = ts
            .client
            .get(&format!("{ROOT_WS}/containers/{}", board.id))
            .await
            .ok_json();
        assert_eq!(
            stored.properties, board.properties,
            "{key}: stored as answered"
        );
    }

    let notes = template(&ts.client, "meeting-notes").await;
    let board = made_on_new_years_eve(&ts, &ts.client, &notes.id).await;
    let days = days_among(&board.blocks);
    assert!(
        days.values().any(|days| !days.is_empty()),
        "the board has days"
    );
    let later = NEW_YEARS_EVE + 90 * 86_400_000;
    let user = ts.client.me().await;
    let (copy, _) = ts
        .app
        .duplicate_container_at(
            &board.containers[0].id,
            &user.id,
            ROOT_WORKSPACE_ID,
            DuplicateAs::Copy,
            None,
            later,
        )
        .await
        .expect("the board is copied");
    assert_eq!(days_among(&copy.blocks), days, "a board's days are its own");
    ts.teardown().await;
}

/// The welcome board is such a board: asked for on the last evening of the year, each
/// of its days is thirty days after the day the template has.
#[tokio::test]
async fn a_welcome_board_has_its_days_moved_to_the_day_it_is_asked_for() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    let written = days_of(&ts.client, &template(&ts.client, "welcome").await.id).await;
    let user = ts.client.me().await;
    let board_id = ts
        .app
        .create_welcome_board_at(&user.id, ROOT_WORKSPACE_ID, None, NEW_YEARS_EVE)
        .await
        .expect("the welcome board is made");
    let moved = days_of(&ts.client, &board_id).await;

    let mut compared = 0;
    for (title, days) in &written {
        for (before, after) in days.iter().zip(&moved[title]) {
            let (Some(before), Some(after)) = (before["from"].as_str(), after["from"].as_str())
            else {
                panic!("a day of {title} is written as a day: {before} and {after}");
            };
            assert_eq!(
                fvoci_model::days_between(before, after),
                Some(30),
                "{title}"
            );
            compared += 1;
        }
        assert_eq!(days.len(), moved[title].len(), "{title}");
    }
    assert!(compared > 0, "the welcome board has days: {written:?}");
    ts.teardown().await;
}
