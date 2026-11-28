//! The search of what cards say against a real search server: Elasticsearch or
//! `OpenSearch` with the nori plugin, named by the environment. Ignored unless asked for:
//!
//! ```text
//! FVOCI_SEARCH_TEST_URL=http://127.0.0.1:19207 \
//!   cargo test -p fvoci-server --test api search_real -- --ignored --nocapture
//! ```
//!
//! With `FVOCI_SEARCH_TEST_USERNAME` and `FVOCI_SEARCH_TEST_PASSWORD` the server signs
//! in with them. The test makes an index of its own name and removes it at the end. It
//! writes cards of a board and pages of a wiki through the API, waits until the server
//! has sent them, and asks the questions of [`QUESTIONS`]: the ones marked as required
//! must find their card or page, and all of them are written, with what they found, to
//! the file `FVOCI_SEARCH_TEST_REPORT` names (a table, for the record of what nori finds
//! as it is configured here). Then a page, the board and the wiki go to the trash, and
//! their documents with them.

use std::fmt::Write as _;
use std::time::Duration;

use fvoci_model::{Container, ContainerKind, ContainerVisibility, SearchHit, SearchKind, new_id};
use fvoci_server::config::search::SearchSettings;
use serde_json::Value;

use crate::boards::new_board;
use crate::common::{Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::content_search::{new_card, search};
use crate::search_pages::{new_page, new_wiki};

/// The titles of [`WRITTEN`] that are pages of a wiki; the others are cards of a board.
const PAGES: [&str; 5] = [
    "고객 응대 안내",
    "휴가 정책",
    "장애 보고서",
    "디자인 시스템",
    "당근마켓 연동",
];

/// What is written: a title and a body.
const WRITTEN: [(&str, &str); 13] = [
    (
        "고객 응대 안내",
        "결제 후 7일이 지나면 환불규정을 따라 처리합니다.",
    ),
    (
        "배포 일정",
        "다음 주 화요일에 서버를 배포합니다. 배포 전 점검표를 확인하세요.",
    ),
    (
        "회의록 11월",
        "검색엔진 도입을 논의했다. 형태소분석기는 nori를 쓴다.",
    ),
    (
        "API 가이드",
        "REST API v2.1의 인증은 OAuth 토큰으로 한다. Rate limit은 분당 60회.",
    ),
    (
        "신입사원 온보딩",
        "입사 첫 주에는 개발환경을 설정하고 팀원들과 점심을 먹습니다.",
    ),
    (
        "휴가 정책",
        "연차휴가는 입사일 기준으로 15일이 주어진다. 반차도 쓸 수 있다.",
    ),
    (
        "장애 보고서",
        "데이터베이스 연결이 끊어져서 로그인할 수 없었습니다. 원인은 커넥션풀 고갈이었다.",
    ),
    (
        "디자인 시스템",
        "버튼의 색상과 글꼴 크기를 정리했다. 다크모드도 지원한다.",
    ),
    (
        "삼성전자 미팅",
        "삼성전자와 서울특별시 강남구에서 미팅. 계약서 초안을 검토했다.",
    ),
    (
        "프로젝트 회고",
        "잘한 점: 일정을 지켰다. 아쉬운 점: 테스트가 부족했다.",
    ),
    (
        "Kubernetes 운영",
        "쿠버네티스 클러스터를 업그레이드했다. Pod가 재시작되었다.",
    ),
    ("맛집 목록", "회사 근처 김치찌개 집과 떡볶이 집."),
    (
        "당근마켓 연동",
        "당근마켓 API로 카카오톡 알림을 보낸다. fvoci 서버와 잇는다.",
    ),
];

/// What a question is to find.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The card of this title, or the test fails: what the search was built for.
    Required(&'static str),
    /// The card of this title, if nori reads the words as hoped: recorded either way.
    Hoped(&'static str),
    /// No card, or the test fails: the words are in none.
    Nothing,
}

/// The questions: what is typed, what it is to find, and what it tries.
const QUESTIONS: [(&str, Expect, &str); 32] = [
    (
        "환불 규정",
        Expect::Required("고객 응대 안내"),
        "붙여 쓴 낱말과 조사",
    ),
    (
        "환불규정",
        Expect::Required("고객 응대 안내"),
        "붙여 쓴 그대로",
    ),
    (
        "환불",
        Expect::Required("고객 응대 안내"),
        "복합어의 앞 낱말",
    ),
    (
        "규정을",
        Expect::Required("고객 응대 안내"),
        "조사를 단 질문",
    ),
    ("배포", Expect::Required("배포 일정"), "제목과 본문의 명사"),
    ("배포하다", Expect::Hoped("배포 일정"), "용언의 다른 활용"),
    (
        "검색 엔진",
        Expect::Required("회의록 11월"),
        "붙여 쓴 복합어를 띄어서",
    ),
    ("검색엔진", Expect::Required("회의록 11월"), "복합어 그대로"),
    (
        "형태소 분석",
        Expect::Hoped("회의록 11월"),
        "세 낱말 복합어의 일부",
    ),
    (
        "nori",
        Expect::Required("회의록 11월"),
        "한글 사이의 영어 낱말",
    ),
    (
        "API 인증",
        Expect::Required("API 가이드"),
        "영어와 한국어 섞임",
    ),
    ("v2.1", Expect::Required("API 가이드"), "판 번호"),
    ("oauth", Expect::Required("API 가이드"), "소문자로 친 영어"),
    ("rate limit", Expect::Required("API 가이드"), "영어 두 낱말"),
    (
        "개발 환경",
        Expect::Required("신입사원 온보딩"),
        "붙여 쓴 복합어와 조사",
    ),
    (
        "먹었다",
        Expect::Hoped("신입사원 온보딩"),
        "용언의 다른 시제",
    ),
    ("휴가", Expect::Required("휴가 정책"), "복합어의 뒤 낱말"),
    ("연차", Expect::Hoped("휴가 정책"), "복합어의 앞 낱말"),
    ("로그인", Expect::Required("장애 보고서"), "외래어와 용언"),
    (
        "커넥션 풀",
        Expect::Hoped("장애 보고서"),
        "사전에 없을 외래어 복합어",
    ),
    ("다크 모드", Expect::Hoped("디자인 시스템"), "외래어 복합어"),
    ("삼성", Expect::Hoped("삼성전자 미팅"), "고유 명사의 일부"),
    (
        "강남",
        Expect::Hoped("삼성전자 미팅"),
        "행정 구역 이름의 일부",
    ),
    (
        "쿠버네티스",
        Expect::Required("Kubernetes 운영"),
        "외래어 한글 표기",
    ),
    (
        "kubernetes",
        Expect::Required("Kubernetes 운영"),
        "제목의 영어, 소문자로",
    ),
    ("김치 찌개", Expect::Hoped("맛집 목록"), "붙여 쓴 음식 이름"),
    (
        "당근마켓",
        Expect::Required("당근마켓 연동"),
        "사전에 없는 상호",
    ),
    (
        "당근",
        Expect::Hoped("당근마켓 연동"),
        "사전에 없는 상호의 일부",
    ),
    (
        "fvoci",
        Expect::Required("당근마켓 연동"),
        "사전에 없는 영어 이름",
    ),
    (
        "카톡",
        Expect::Hoped("당근마켓 연동"),
        "줄임말(동의어가 있어야 찾는다)",
    ),
    ("환급", Expect::Nothing, "글자가 비슷한 다른 낱말"),
    ("배", Expect::Nothing, "낱말의 첫 글자"),
];

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// A request to the search server itself, signed in as the server is.
fn direct(
    settings: &SearchSettings,
    method: reqwest::Method,
    path: &str,
) -> reqwest::RequestBuilder {
    let request = reqwest::Client::new()
        .request(method, format!("{}{path}", settings.search_url))
        .timeout(Duration::from_secs(30));
    if settings.search_username.is_empty() {
        request
    } else {
        request.basic_auth(&settings.search_username, Some(&settings.search_password))
    }
}

/// Has the upkeep send what changed until nothing is left, and the index show it.
async fn sent_and_shown(th: &TestServer, settings: &SearchSettings) {
    let everyone = th.app.housekeeping();
    for _ in 0..50 {
        fvoci_server::search::tend(&th.app).await;
        let counts = everyone.store().search_outbox_counts().await;
        if counts.expect("counted").waiting == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let index = &settings.search_index;
    direct(
        settings,
        reqwest::Method::POST,
        &format!("/{index}/_refresh"),
    )
    .send()
    .await
    .expect("the index is refreshed");
}

/// How many documents the index of the test holds.
async fn count(settings: &SearchSettings) -> usize {
    let index = &settings.search_index;
    let counted: Value = direct(settings, reqwest::Method::GET, &format!("/{index}/_count"))
        .send()
        .await
        .expect("the search server counts")
        .json()
        .await
        .expect("a count");
    usize::try_from(counted["count"].as_u64().expect("a number")).expect("a count")
}

/// A snippet with what matched between brackets.
fn shown(hit: &SearchHit) -> String {
    hit.snippet.iter().fold(String::new(), |mut text, part| {
        if part.matched {
            let _ = write!(text, "[{}]", part.text);
        } else {
            text.push_str(&part.text);
        }
        text
    })
}

#[tokio::test]
#[ignore = "needs a search server with nori, named by FVOCI_SEARCH_TEST_URL"]
#[expect(
    clippy::too_many_lines,
    reason = "one run against one search server: write, wait, ask, record"
)]
async fn korean_questions_find_their_cards_on_a_real_search_server() {
    let url = env("FVOCI_SEARCH_TEST_URL").expect("FVOCI_SEARCH_TEST_URL names the search server");
    let settings = SearchSettings {
        search_url: url,
        search_username: env("FVOCI_SEARCH_TEST_USERNAME").unwrap_or_default(),
        search_password: env("FVOCI_SEARCH_TEST_PASSWORD").unwrap_or_default(),
        search_index: format!("fvoci-test-{}", &new_id()[24..]),
        search_requests_per_second: 0,
        upkeep_by_hand: true,
        ..SearchSettings::default()
    };
    let version: Value = direct(&settings, reqwest::Method::GET, "/")
        .send()
        .await
        .expect("the search server answers")
        .json()
        .await
        .expect("what it is");
    let flavor = format!(
        "{} {}",
        version["version"]["distribution"]
            .as_str()
            .unwrap_or("elasticsearch"),
        version["version"]["number"].as_str().unwrap_or("?")
    );
    let th = Setup::native()
        .search(settings.clone())
        .start()
        .await
        .init_basic()
        .await;
    fvoci_server::search::tend(&th.app).await;
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "한국어",
        ))
        .await
        .created_json();
    let wiki = new_wiki(&th.client).await;
    let mut refund_page = String::new();
    for (title, text) in WRITTEN {
        if PAGES.contains(&title) {
            let page = new_page(&th.client, &wiki, "", title, text).await;
            if refund_page.is_empty() {
                refund_page = page.id;
            }
        } else {
            new_card(&th.client, &board.id, title, text).await;
        }
    }
    // A body as long as one may be: longer than what the highlighter of either search
    // server reads unless the index says otherwise.
    let word = " zyzzyva";
    let line = "refund policy words ";
    // The JSON around the text of a body of one paragraph.
    let frame =
        r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":""}]}]}"#;
    let room = fvoci_model::BODY_MAX_BYTES - frame.len() - word.len();
    let long = format!(
        "{}{}{word}",
        line.repeat(room / line.len()),
        "x".repeat(room % line.len())
    );
    assert_eq!(long.len() + frame.len(), fvoci_model::BODY_MAX_BYTES);
    new_card(&th.client, &board.id, "긴 본문", &long).await;
    sent_and_shown(&th, &settings).await;
    let in_the_long_one: Vec<SearchHit> = search(&th.client, TEST_WS, "zyzzyva").await.ok_json();

    let mut table = format!(
        "### {flavor}\n\n| 질문 | 보려는 것 | 찾을 것 | 바란 대로인가 | 찾은 것(순서대로) | 조각 |\n|---|---|---|---|---|---|\n"
    );
    let mut missed = Vec::new();
    for (question, expect, tries) in QUESTIONS {
        let hits: Vec<SearchHit> = search(&th.client, TEST_WS, question).await.ok_json();
        let titles: Vec<&str> = hits.iter().map(|hit| hit.title.as_str()).collect();
        let (wanted, required) = match expect {
            Expect::Required(title) => (title, true),
            Expect::Hoped(title) => (title, false),
            Expect::Nothing => {
                if !hits.is_empty() {
                    missed.push(question);
                }
                let _ = writeln!(
                    table,
                    "| {question} | {tries} | (없음) | {} | {} | |",
                    if hits.is_empty() { "예" } else { "아니오" },
                    titles.join(", "),
                );
                continue;
            }
        };
        let (kind, word) = if PAGES.contains(&wanted) {
            (SearchKind::Page, "페이지")
        } else {
            (SearchKind::Card, "카드")
        };
        let hit = hits
            .iter()
            .find(|hit| hit.title == wanted && hit.kind == kind);
        if required && hit.is_none() {
            missed.push(question);
        }
        let _ = writeln!(
            table,
            "| {question} | {tries} | {wanted} ({word}){} | {} | {} | {} |",
            if required { "" } else { ", 바람" },
            if hit.is_some() { "예" } else { "아니오" },
            titles.join(", "),
            hit.map_or_default(shown),
        );
    }
    // The case the search was asked for, with the piece of the body that matched.
    let refund: Vec<SearchHit> = search(&th.client, TEST_WS, "환불 규정").await.ok_json();
    // Its page goes to the trash, and then the board and the wiki with the others: the
    // search server holds none of them any more.
    th.client
        .delete(&format!("{TEST_WS}/wikis/{wiki}/pages/{refund_page}"))
        .await
        .check_no_content();
    sent_and_shown(&th, &settings).await;
    let trashed: Vec<SearchHit> = search(&th.client, TEST_WS, "환불 규정").await.ok_json();
    let others = count(&settings).await;
    for container in [&board.id, &wiki] {
        th.client
            .delete(&format!("{TEST_WS}/containers/{container}"))
            .await
            .check_no_content();
    }
    sent_and_shown(&th, &settings).await;
    let none = count(&settings).await;
    let removed = direct(
        &settings,
        reqwest::Method::DELETE,
        &format!("/{}", settings.search_index),
    )
    .send()
    .await
    .expect("the index of the test is removed");
    th.teardown().await;

    if let Some(report) = env("FVOCI_SEARCH_TEST_REPORT") {
        std::fs::write(&report, &table).expect("the table is written");
    }
    assert!(removed.status().is_success(), "{}", removed.status());
    assert_eq!(
        in_the_long_one
            .iter()
            .map(|hit| hit.title.as_str())
            .collect::<Vec<_>>(),
        ["긴 본문"],
        "a word at the end of the longest body is found, and the search answered"
    );
    assert!(
        in_the_long_one[0].snippet.iter().any(|part| part.matched),
        "and marked in its piece"
    );
    assert_eq!(trashed, [], "a page in the trash is found no more");
    assert_eq!(
        others,
        WRITTEN.len(),
        "its document was removed, and no other"
    );
    assert_eq!(
        none, 0,
        "the documents of what is in the trash were removed"
    );
    assert_eq!(missed, Vec::<&str>::new(), "on {flavor}:\n{table}");
    let refund = refund.first().expect("the page of the refund rules");
    assert_eq!(refund.title, "고객 응대 안내");
    assert_eq!(
        (refund.kind, refund.container_kind, &refund.container_id),
        (SearchKind::Page, ContainerKind::Wiki, &wiki)
    );
    assert!(
        refund
            .snippet
            .iter()
            .any(|part| part.matched && part.text.contains("환불")),
        "the piece of the body marks what matched: {:?}",
        refund.snippet
    );
}
