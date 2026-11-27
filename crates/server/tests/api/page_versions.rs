//! The versions kept of a page's body: who reads them and who goes back to one, by the
//! roles of the wiki, as a card's; and that they are reached through their own page
//! while it is not in the trash.

use serde_json::{Value, json};

use reqwest::StatusCode;

use crate::common::{ApiResponse, Client, ROOT_WS, Setup, TEST_WS, UNKNOWN_ID};
use crate::wiki_isolation::{put_on, wiki_of};

/// A body of one paragraph.
fn text(said: &str) -> Value {
    json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": said}
    ]}]})
}

fn page_path(at: &str, wiki: &str, page: &str) -> String {
    format!("{at}/wikis/{wiki}/pages/{page}")
}

/// Writes the bodies to the page one after the other, as `client`.
async fn write(client: &Client, wiki: &str, page: &str, bodies: &[&str]) {
    let at = page_path(TEST_WS, wiki, page);
    for body in bodies {
        let read = client.get(&at).await.value();
        client
            .patch(
                &at,
                &json!({"readAt": read["updatedAt"], "body": text(body)}),
            )
            .await
            .check_ok();
    }
}

/// The number of the version whose body says `said`, as `client` reads them.
async fn version_saying(client: &Client, wiki: &str, page: &str, said: &str) -> i64 {
    let at = format!("{}/body-versions", page_path(TEST_WS, wiki, page));
    let listed = client.get(&at).await.value();
    for listed in listed.as_array().expect("a list") {
        let number = listed["version"].as_i64().expect("a number");
        let read = client.get(&format!("{at}/{number}")).await.value();
        if read["body"] == text(said) {
            return number;
        }
    }
    panic!("no version says {said:?}: {listed}");
}

/// What the three operations of versions answer, in this order: the list, a version,
/// going back to it.
async fn answers(
    client: &Client,
    at: &str,
    wiki: &str,
    page: &str,
    version: i64,
) -> [ApiResponse; 3] {
    let versions = format!("{}/body-versions", page_path(at, wiki, page));
    [
        client.get(&versions).await,
        client.get(&format!("{versions}/{version}")).await,
        client
            .post_empty(&format!("{versions}/{version}/restore"))
            .await,
    ]
}

/// The statuses of [`answers`].
async fn asked(client: &Client, at: &str, wiki: &str, page: &str, version: i64) -> [u16; 3] {
    answers(client, at, wiki, page, version)
        .await
        .map(|response| response.status.as_u16())
}

/// The problems of [`answers`], as a refusal is compared with another.
async fn refused(
    client: &Client,
    at: &str,
    wiki: &str,
    page: &str,
    version: i64,
) -> Vec<(StatusCode, Value)> {
    answers(client, at, wiki, page, version)
        .await
        .iter()
        .map(ApiResponse::problem)
        .collect()
}

/// Every member of the wiki reads the versions; its editors go back to one, which is
/// then the page's body, and its newest version.
#[tokio::test]
async fn the_wikis_members_read_the_versions_and_its_editors_go_back_to_one() {
    let th = Setup::native().start().await.init_basic().await;
    let member = th.client2.me().await.id;
    let (wiki, page) = wiki_of(&th.client, "private").await;
    write(&th.client, &wiki, &page, &["first", "second"]).await;
    let first = version_saying(&th.client, &wiki, &page, "first").await;

    for (role, expected) in [("viewer", [200, 200, 403]), ("commenter", [200, 200, 403])] {
        put_on(&th.client, &wiki, &member, role).await;
        assert_eq!(
            asked(&th.client2, TEST_WS, &wiki, &page, first).await,
            expected,
            "{role}"
        );
    }
    let read = th.client.get(&page_path(TEST_WS, &wiki, &page)).await;
    assert_eq!(
        read.value()["body"],
        text("second"),
        "refused, nothing changed"
    );
    let listed = format!("{}/body-versions", page_path(TEST_WS, &wiki, &page));
    let before = th.client.get(&listed).await.value();
    assert_eq!(
        before.as_array().map(Vec::len),
        Some(2),
        "and no version kept: {before}"
    );

    put_on(&th.client, &wiki, &member, "editor").await;
    let restored = th
        .client2
        .post_empty(&format!(
            "{}/body-versions/{first}/restore",
            page_path(TEST_WS, &wiki, &page)
        ))
        .await;
    restored.check_ok();
    let restored = restored.value();
    assert_eq!(
        (&restored["id"], &restored["body"], &restored["modifiedBy"]),
        (&json!(page), &text("first"), &json!(member)),
        "the page as it is now"
    );
    let versions = th
        .client
        .get(&format!(
            "{}/body-versions",
            page_path(TEST_WS, &wiki, &page)
        ))
        .await
        .value();
    let newest = &versions[0];
    assert_eq!(newest["isCurrent"], json!(true), "{versions}");
    assert_eq!(newest["editors"], json!([member]), "{versions}");
    // And the wiki's admin, to the second again.
    let second = version_saying(&th.client, &wiki, &page, "second").await;
    let again = th
        .client
        .post_empty(&format!("{listed}/{second}/restore"))
        .await;
    again.check_ok();
    assert_eq!(again.value()["body"], text("second"));
    th.teardown().await;
}

/// Who is not on the wiki, and who names it under another workspace, is answered as
/// for a wiki that is nowhere; a number the page has no version of is no version; and
/// a page in the trash has none to give.
#[tokio::test]
async fn the_versions_are_reached_through_their_page_while_it_is_there() {
    let th = Setup::native().start().await.init_basic().await;
    let (wiki, page) = wiki_of(&th.client, "private").await;
    write(&th.client, &wiki, &page, &["first", "second"]).await;
    let first = version_saying(&th.client, &wiki, &page, "first").await;
    let none = asked(&th.client2, TEST_WS, UNKNOWN_ID, UNKNOWN_ID, first).await;
    assert_eq!(none, [404; 3]);
    for (client, at) in [(&th.client2, TEST_WS), (&th.client, ROOT_WS)] {
        assert_eq!(asked(client, at, &wiki, &page, first).await, none, "{at}");
        // To the letter, for a version that is there and for one that is not.
        let of_none = refused(client, at, UNKNOWN_ID, &page, 999).await;
        for version in [first, 999] {
            let of_wiki = refused(client, at, &wiki, &page, version).await;
            assert_eq!(of_wiki, of_none, "{at}, version {version}");
        }
    }
    // Another page of the same wiki has versions of its own: the first's numbers name
    // none of it, through its path.
    let sibling = th
        .client
        .post(
            &format!("{TEST_WS}/wikis/{wiki}/pages"),
            &json!({"title": "sibling"}),
        )
        .await;
    sibling.check_created();
    let sibling = sibling.value()["id"].as_str().expect("an id").to_owned();
    assert_eq!(
        asked(&th.client, TEST_WS, &wiki, &sibling, first).await,
        [200, 404, 404],
        "the first page's version through the sibling's path"
    );
    // A number is a number.
    let (other_wiki, other_page) = wiki_of(&th.client, "private").await;
    let at = format!("{}/body-versions", page_path(TEST_WS, &wiki, &page));
    for missing in ["999", "one", "-1"] {
        th.client
            .get(&format!("{at}/{missing}"))
            .await
            .check_not_found();
        th.client
            .post_empty(&format!("{at}/{missing}/restore"))
            .await
            .check_not_found();
    }
    assert_eq!(
        asked(&th.client, TEST_WS, &other_wiki, &page, first).await,
        [404; 3],
        "the page under another wiki"
    );
    let theirs = format!(
        "{}/body-versions",
        page_path(TEST_WS, &other_wiki, &other_page)
    );
    let listed = th.client.get(&theirs).await.value();
    assert_eq!(listed, json!([]), "a page never written has no versions");

    th.client
        .delete(&page_path(TEST_WS, &wiki, &page))
        .await
        .check_no_content();
    assert_eq!(
        asked(&th.client, TEST_WS, &wiki, &page, first).await,
        [404; 3],
        "in the trash"
    );
    // A page of a wiki in the trash has none either.
    th.client
        .delete(&format!("{TEST_WS}/containers/{other_wiki}"))
        .await
        .check_no_content();
    assert_eq!(
        asked(&th.client, TEST_WS, &other_wiki, &other_page, 1).await,
        [404; 3],
        "the wiki in the trash"
    );
    th.teardown().await;
}
