//! How long a search of the database takes in a workspace of ten thousand cards, each
//! with a body of about two kilobytes of Korean that does not repeat itself. Ignored
//! unless asked for, on the database of the tests:
//!
//! ```text
//! cargo test -p fvoci-server --test api search_database_size -- --ignored --nocapture
//! ```
//!
//! It prints a table: each question, how many it found, and the fastest and the
//! slowest of five searches through the API (the check of every hit included).

use std::time::{Duration, Instant};

use fvoci_model::{Block, BlockType, Body, ContainerVisibility, SearchHit, new_id};
use fvoci_server::config::search::SearchSettings;
use serde_json::json;

use crate::common::{Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::content_search::search;

/// How many cards the board has.
const CARDS: usize = 10_000;
/// About how many characters a body has: two kilobytes of Hangul and spaces.
const BODY_CHARS: usize = 820;
/// How many cards are stored in one transaction.
const AT_ONCE: usize = 500;
/// How many times each question is asked.
const TIMES: usize = 5;

/// A generator of numbers that are the same at every run.
struct Numbers(u64);

impl Numbers {
    fn next(&mut self, below: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let high = usize::try_from(self.0 >> 33).expect("31 bits fit");
        high % below
    }
}

/// A word of two or three syllables, of the syllables of Hangul.
fn word(numbers: &mut Numbers) -> String {
    let syllables = 2 + numbers.next(2);
    (0..syllables)
        .map(|_| {
            let code = 0xAC00 + u32::try_from(numbers.next(11_172)).expect("a syllable");
            char::from_u32(code).expect("a syllable of Hangul")
        })
        .collect()
}

/// The body of the `index`th card: words of a vocabulary of thousands, in an order of
/// their own; every hundredth card also says "드문낱말", and every card that is even
/// "흔한말". One in ten says "Release" in some case.
fn text(numbers: &mut Numbers, vocabulary: &[String], index: usize) -> String {
    let mut text = String::new();
    if index.is_multiple_of(100) {
        text.push_str("드문낱말 ");
    }
    if index.is_multiple_of(2) {
        text.push_str("흔한말 ");
    }
    if index.is_multiple_of(10) {
        text.push_str(if index.is_multiple_of(20) {
            "RELEASE "
        } else {
            "release "
        });
    }
    while text.chars().count() < BODY_CHARS {
        text.push_str(&vocabulary[numbers.next(vocabulary.len())]);
        text.push(if numbers.next(12) == 0 { '\n' } else { ' ' });
    }
    text
}

#[tokio::test]
#[ignore = "takes minutes: run it to measure"]
async fn a_search_of_ten_thousand_cards_in_the_database() {
    let th = Setup::native()
        .search(SearchSettings {
            search_requests_per_second: 0,
            ..SearchSettings::default()
        })
        .start()
        .await
        .init_basic()
        .await;
    let user = th.client.me().await.id;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let mut numbers = Numbers(7);
    let vocabulary: Vec<String> = (0..5000).map(|_| word(&mut numbers)).collect();
    let stored = Instant::now();
    let mut bytes = 0;
    for start in (0..CARDS).step_by(AT_ONCE) {
        let mut blocks: Vec<Block> = (start..start + AT_ONCE)
            .map(|index| {
                let text = text(&mut numbers, &vocabulary, index);
                bytes += text.len();
                let paragraphs: Vec<_> = text
                    .lines()
                    .map(|line| json!({"type": "paragraph", "content": [{"type": "text", "text": line}]}))
                    .collect();
                Block {
                    id: new_id(),
                    container_id: board.id.clone(),
                    block_type: BlockType::Card,
                    title: format!("카드 {index}"),
                    body: Body::parse(json!({"type": "doc", "content": paragraphs}))
                        .expect("a body that may be stored"),
                    ..Block::default()
                }
            })
            .collect();
        th.app
            .store()
            .insert_blocks(&mut blocks, &user)
            .await
            .expect("the cards are stored");
    }
    println!(
        "stored {CARDS} cards, {} bytes of body on average, in {:?}",
        bytes / CARDS,
        stored.elapsed()
    );

    let questions = [
        "없는낱말",
        "드문낱말",
        "흔한말",
        "흔한말 드문낱말",
        "release",
        &vocabulary[0],
        "카드 9999",
    ];
    println!("| question | found | fastest | slowest |\n|---|---|---|---|");
    for question in questions {
        let mut times: Vec<Duration> = Vec::with_capacity(TIMES);
        let mut found = 0;
        for _ in 0..TIMES {
            let asked = Instant::now();
            let answer = search(&th.client, TEST_WS, question).await;
            times.push(asked.elapsed());
            found = answer.ok_json::<Vec<SearchHit>>().len();
        }
        times.sort();
        println!(
            "| {question} | {found} | {:?} | {:?} |",
            times[0],
            times[TIMES - 1]
        );
    }
    th.teardown().await;
}
