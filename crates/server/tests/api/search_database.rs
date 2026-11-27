//! The search of what cards and pages say on a server without a search server
//! (`search_url` empty): the database is searched by the letters of the titles and the
//! bodies, behind the same API. A server that has a search server is never searched
//! so: while it does not answer, a search is a 503.

use std::time::{Duration, Instant};

use fvoci_model::{Container, ContainerVisibility, SearchHit, new_id};
use fvoci_server::config::search::SearchSettings;
use reqwest::StatusCode;

use crate::boards::new_board;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::content_search::{new_card, search, search_with, started, stored_card};
use crate::fake_search::FakeSearch;
use crate::search_pages::{new_page, new_wiki};

/// The identifiers a search found, in its order.
fn ids(hits: &[SearchHit]) -> Vec<String> {
    hits.iter().map(|hit| hit.id.clone()).collect()
}

/// What a search finds, in its order.
async fn hits(client: &Client, parameters: &[(&str, &str)]) -> Vec<SearchHit> {
    search_with(client, TEST_WS, parameters).await.ok_json()
}

/// Says that a card or a page was last changed at `at`, so that the order of the
/// most recently changed is known.
async fn changed_at(th: &TestServer, id: &str, at: i64) {
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE blocks SET updated_at = {at} WHERE id = '{id}'"
        ))
        .await
        .expect("the time of the change is set");
}

/// The stretches of a snippet.
fn shown(hit: &SearchHit) -> Vec<(&str, bool)> {
    hit.snippet
        .iter()
        .map(|part| (part.text.as_str(), part.matched))
        .collect()
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "every way the letters are matched, on one board"
)]
async fn without_a_search_server_the_letters_of_titles_and_bodies_are_found() {
    // Many searches in a row: no limit to how often.
    let th = Setup::native()
        .search(SearchSettings {
            search_requests_per_second: 0,
            ..SearchSettings::default()
        })
        .start()
        .await
        .init_basic()
        .await;
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Support",
        ))
        .await
        .created_json();
    let made = async |title: &str, text: &str, at: i64| {
        let card = new_card(&th.client, &board.id, title, text).await;
        changed_at(&th, &card.id, at).await;
        card.id
    };
    // The title holds the word: first, though changed longest ago.
    let titled = made("환불 안내", "7일 안에 신청한다", 1000).await;
    let rule = made("고객 응대", "결제 후 환불규정을 따른다", 2000).await;
    let asked = made("문의", "환불 문의가 왔다", 3000).await;
    let deploy = made("배포 일정", "다음 주에 배포합니다", 4000).await;
    // Each says the word in one case only, in its body alone.
    let api = made("Guide", "The REST API uses tokens", 5000).await;
    let lower_api = made("Notes", "an api of ours", 4500).await;
    let summer = made("Été", "plage", 6000).await;
    let signs = made("할인율", r"100% 할인_코드 C:\temp", 7000).await;
    let hundred = made("인원", "100명 할인 코드", 8000).await;
    let wiki = new_wiki(&th.client).await;
    let page = new_page(&th.client, &wiki, "", "쪽", "환불 절차를 적는다").await;
    changed_at(&th, &page.id, 500).await;

    let config = th.anonymous().get("/client-config").await.value();
    let refund = hits(&th.client, &[("q", "환불")]).await;
    let every_word = hits(&th.client, &[("q", "환불  규정")]).await;
    let not_an_ending = hits(&th.client, &[("q", "배포하다")]).await;
    let a_stem = hits(&th.client, &[("q", "배포")]).await;
    let upper = hits(&th.client, &[("q", "API")]).await;
    let lower = hits(&th.client, &[("q", "api")]).await;
    let accents = [
        hits(&th.client, &[("q", "été")]).await,
        hits(&th.client, &[("q", "ÉTÉ")]).await,
        hits(&th.client, &[("q", "Été")]).await,
    ];
    let percent = hits(&th.client, &[("q", "100%")]).await;
    let underscore = hits(&th.client, &[("q", "_")]).await;
    let backslash = hits(&th.client, &[("q", r"\")]).await;
    let pages = hits(&th.client, &[("q", "환불"), ("kind", "page")]).await;
    let cards = hits(&th.client, &[("q", "환불"), ("kind", "card")]).await;
    let one = hits(&th.client, &[("q", "환불"), ("limit", "1")]).await;
    let nul = search(&th.client, TEST_WS, "환\0불").await;
    let blank = hits(&th.client, &[("q", "   ")]).await;
    // The second user reads a board of their own, which does not hold the word.
    let theirs: Container = th
        .client2
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Theirs",
        ))
        .await
        .created_json();
    new_card(&th.client2, &theirs.id, "다른 말", "다른 말").await;
    let unjoined = hits(&th.client2, &[("q", "환불")]).await;
    th.teardown().await;

    assert_eq!(config["searchEnabled"], true, "{config}");
    assert_eq!(
        ids(&refund),
        [titled.clone(), asked.clone(), rule.clone(), page.id.clone()],
        "the title first, then the most recently changed, a page among the cards"
    );
    let rule_hit = refund.iter().find(|hit| hit.id == rule).expect("found");
    assert_eq!(
        shown(rule_hit),
        [
            ("결제 후 ", false),
            ("환불", true),
            ("규정을 따른다", false)
        ]
    );
    assert_eq!(rule_hit.title, "고객 응대");
    assert_eq!(rule_hit.container_id, board.id);
    assert_eq!(
        ids(&every_word),
        std::slice::from_ref(&rule),
        "every word, in the title or the body"
    );
    assert_eq!(
        ids(&not_an_ending),
        Vec::<String>::new(),
        "no ending is taken off"
    );
    assert_eq!(ids(&a_stem), [deploy]);
    let both = [api, lower_api];
    assert_eq!(ids(&upper), both, "the upper case finds the lower");
    assert_eq!(ids(&lower), both, "and the lower the upper");
    assert_eq!(
        shown(&lower[0]),
        [("The REST ", false), ("API", true), (" uses tokens", false)]
    );
    assert_eq!(
        accents.map(|found| ids(&found)),
        [vec![], vec![], vec![summer]],
        "ASCII letters alone are found in either case"
    );
    assert_eq!(ids(&percent), std::slice::from_ref(&signs), "a % is a %");
    assert_eq!(
        ids(&underscore),
        std::slice::from_ref(&signs),
        "an _ is an _"
    );
    assert_eq!(ids(&backslash), [signs], r"a \ is a \");
    assert!(!ids(&percent).contains(&hundred));
    assert_eq!(ids(&pages), std::slice::from_ref(&page.id));
    assert_eq!(cards.len(), 3);
    assert_eq!(ids(&one), std::slice::from_ref(&titled));
    assert_eq!(nul.ok_json::<Vec<SearchHit>>(), [], "no text holds a NUL");
    assert_eq!(blank, []);
    assert_eq!(
        ids(&unjoined),
        [titled, asked, rule],
        "an open board is searched without joining, while the private wiki stays hidden"
    );
}

#[tokio::test]
async fn a_search_of_the_database_that_runs_past_its_time_is_a_503() {
    let th = Setup::native()
        .search(SearchSettings {
            timeout: Duration::ZERO,
            ..SearchSettings::default()
        })
        .start()
        .await
        .init_basic()
        .await;
    th.create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;

    let late = search(&th.client, TEST_WS, "환불").await;
    let me = th.client.get("/users/me").await;
    th.teardown().await;

    late.check_status(StatusCode::SERVICE_UNAVAILABLE);
    let problem = late.check_problem();
    assert_eq!(problem["reason"], "search_unavailable", "{problem}");
    assert_eq!(problem["type"], "/problems/search-unavailable");
    me.check_ok();
}

#[tokio::test]
async fn a_search_server_that_is_down_is_not_stood_in_for_by_the_database() {
    let fake = FakeSearch::start().await;
    let th = started(&fake).await;
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Open",
        ))
        .await
        .created_json();
    new_card(&th.client, &board.id, "환불 안내", "환불규정").await;

    fake.go_down();
    let down = search(&th.client, TEST_WS, "환불").await;
    let config = th.anonymous().get("/client-config").await.value();
    th.teardown().await;

    down.check_status(StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(down.check_problem()["reason"], "search_unavailable");
    assert_eq!(config["searchEnabled"], true, "{config}");
}

#[tokio::test]
async fn a_user_searches_the_database_only_so_often() {
    let th = Setup::native()
        .search(SearchSettings {
            search_requests_per_second: 1,
            ..SearchSettings::default()
        })
        .start()
        .await
        .init_basic()
        .await;
    th.create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;

    // One a second, and two at once after a pause: of eight at once, some wait.
    let sent = Instant::now();
    let answers =
        futures_util::future::join_all((0..8).map(|_| search(&th.client, TEST_WS, "환불"))).await;
    // Two at once, and one more for every second the eight took.
    let most = 2 + usize::try_from(sent.elapsed().as_secs()).expect("seconds") + 1;
    let other = search(&th.client2, TEST_WS, "환불").await;
    th.teardown().await;

    let limited = answers
        .iter()
        .filter(|answer| answer.status == StatusCode::TOO_MANY_REQUESTS)
        .count();
    let answered = answers
        .iter()
        .filter(|answer| answer.status == StatusCode::OK)
        .count();
    assert_eq!(limited + answered, 8, "answered, or told to wait");
    assert!(answered >= 1 && limited >= 1, "{answered} answered");
    assert!(answered <= most, "{answered} answered, of at most {most}");
    other.check_ok();
}

#[tokio::test]
async fn a_user_of_more_boards_than_a_search_server_is_asked_about_finds_in_them_all() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await.id;
    let mut last = String::new();
    for number in 0..1001 {
        let mut board = new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            &format!("b{number}"),
        );
        board.id = new_id();
        th.app
            .store()
            .insert_container_with_admin(&mut board, &user1)
            .await
            .expect("a board is stored");
        last = board.id;
    }
    let mine = new_card(&th.client, &last, "내 카드", "환불").await;
    // Of the same workspace, on a board the user is not on.
    let stranger = th.stranger_in(TEST_WORKSPACE_ID).await;
    let theirs = crate::boards::board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "Theirs"),
        &stranger,
    )
    .await;
    stored_card(th.app.store(), &theirs.id, "남의 환불 카드").await;

    let found = hits(&th.client, &[("q", "환불"), ("limit", "7")]).await;
    th.teardown().await;

    assert_eq!(
        ids(&found),
        [mine.id],
        "the last of a thousand and one boards"
    );
}
