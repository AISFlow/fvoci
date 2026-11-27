//! A wiki read by its public link: its tree, its pages and its files, and nothing of
//! anybody. No answer holds the identifier of a user or of the workspace, a reference
//! keeps its identifier for a page of the wiki and goes out empty for anything else, a
//! board's routes of the link (its blocks and cards, where comments would be) are not
//! there for the wiki's token, nor the wiki's for a board's, and the link opens
//! nothing once it is off, the organization's public links are off, or the wiki is in
//! the trash.

use reqwest::Method;
use reqwest::multipart::{Form, Part};
use serde_json::{Value, json};

use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer, UNKNOWN_ID};
use crate::presence::{people, visit};
use crate::references::made;
use crate::references_shared::references_in;
use crate::references_to::body_naming;
use crate::websocket::{QUIET, Socket};

/// A shared wiki whose first page, by its owner, names a page of the wiki and much
/// that is not of it, and mentions both its writers; a second page is by another
/// member. An image of the wiki is uploaded.
struct SharedWiki {
    wiki: String,
    token: String,
    page: String,
    other_page: String,
    /// A page of the wiki in the trash.
    gone: String,
    file: String,
    /// The identifiers no answer may hold: the writers, the workspace, what is
    /// elsewhere.
    secrets: Vec<String>,
    /// What the first page's references name to whoever reads by the link.
    kept: Vec<String>,
}

async fn shared_wiki(th: &TestServer) -> SharedWiki {
    let owner = &th.client;
    let other = &th.client2;
    let mine = made(owner, TEST_WS, "private", "shared").await;
    let elsewhere = made(owner, TEST_WS, "private", "elsewhere").await;
    let owner_id = owner.me().await.id;
    let other_id = other.me().await.id;
    let wiki = mine.wiki.clone();
    let at = format!("{TEST_WS}/wikis/{wiki}/pages");
    owner
        .post(
            &format!("{TEST_WS}/containers/{wiki}/members"),
            &json!({"containerId": wiki, "userId": other_id, "role": "editor"}),
        )
        .await
        .check_ok();
    let other_page = other
        .post(&at, &json!({"title": "by another"}))
        .await
        .value()["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let gone = owner.post(&at, &json!({"title": "gone"})).await.value()["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    owner
        .delete(&format!("{at}/{gone}"))
        .await
        .check_no_content();
    let outside = [
        elsewhere.page.clone(),
        elsewhere.card.clone(),
        mine.card.clone(),
        gone.clone(),
        UNKNOWN_ID.to_owned(),
    ];
    let mut named = vec![other_page.as_str()];
    named.extend(outside.iter().map(String::as_str));
    let mut body = body_naming("see", &named);
    let mention = |id: &str| json!({"type": "mention", "attrs": {"userId": id}});
    body["content"][0]["content"]
        .as_array_mut()
        .expect("a line")
        .extend([mention(&owner_id), mention(&other_id)]);
    let path = format!("{at}/{}", mine.page);
    let read_at = owner.get(&path).await.value()["updatedAt"].clone();
    owner
        .patch(&path, &json!({"readAt": read_at, "body": body}))
        .await
        .check_ok();
    let file = Part::bytes(b"an image of the wiki".to_vec()).file_name("wiki.png");
    let uploaded = Client::send(
        owner
            .request(Method::POST, &format!("{TEST_WS}/containers/{wiki}/files"))
            .multipart(Form::new().part("file", file)),
    )
    .await
    .value();
    let mut kept = vec![other_page.clone()];
    kept.extend(outside.iter().map(|_| String::new()));
    let mut secrets = vec![owner_id, other_id, TEST_WORKSPACE_ID.to_owned()];
    secrets.extend(outside);
    SharedWiki {
        token: th.share(&wiki).await,
        wiki,
        page: mine.page,
        other_page,
        gone,
        file: uploaded["fileId"].as_str().expect("a file").to_owned(),
        secrets,
        kept,
    }
}

impl SharedWiki {
    fn at(&self, rest: &str) -> String {
        let joint = if rest.contains('?') { '&' } else { '?' };
        format!("/shared/{}{rest}{joint}readToken={}", self.wiki, self.token)
    }
}

/// Whoever reads the wiki by its link, with a session or without, reads the wiki, its
/// tree, its pages and its file; no answer holds a writer, the workspace, or anything
/// not of the wiki, and the references keep only the wiki's own pages.
#[tokio::test]
async fn a_wiki_is_read_by_its_link_and_names_nobody_and_nothing_else() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let shared = shared_wiki(&th).await;
    let anonymous = th.anonymous();
    for reader in [&anonymous, &th.client2, &th.client] {
        let mut answers = Vec::new();
        for rest in [
            String::new(),
            "/pages".to_owned(),
            format!("/pages/{}", shared.page),
            format!("/pages/{}", shared.other_page),
            format!("/files/{}/info", shared.file),
        ] {
            let answer = reader.get(&shared.at(&rest)).await;
            answer.check_ok();
            answers.push((rest, answer.text()));
        }
        for (rest, text) in &answers {
            for secret in &shared.secrets {
                assert!(
                    !text.contains(secret.as_str()),
                    "{rest} tells {secret}: {text}"
                );
            }
        }
        let page: Value = serde_json::from_str(&answers[2].1).expect("a page");
        assert_eq!(references_in(&page["body"]), shared.kept);
        assert_eq!(page["createdBy"], "");
        assert_eq!(page["modifiedBy"], "");
        let tree: Value = serde_json::from_str(&answers[1].1).expect("a tree");
        assert_eq!(
            tree.as_array().map(Vec::len),
            Some(2),
            "the live pages: {tree}"
        );
        let file = reader
            .get(&shared.at(&format!("/files/{}", shared.file)))
            .await;
        file.check_ok();
        assert_eq!(file.body, b"an image of the wiki");
    }
    th.teardown().await;
}

/// What a board's link reads (blocks, cards: where comments are) is not there for a
/// wiki's token, nor a page for a board's token, nor a page of another wiki, nor one
/// in the trash.
#[tokio::test]
async fn a_wiki_link_reads_no_blocks_or_cards_and_a_board_link_no_pages() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let shared = shared_wiki(&th).await;
    let owner = &th.client;
    let board = made(owner, TEST_WS, "private", "a board").await;
    let board_token = th.share(&board.board).await;
    let anonymous = th.anonymous();
    for rest in ["/blocks?all=true", "/cards", "/blocks?type=comment"] {
        anonymous.get(&shared.at(rest)).await.check_not_found();
    }
    for rest in ["pages", &format!("pages/{}", board.card)] {
        anonymous
            .get(&format!(
                "/shared/{}/{rest}?readToken={board_token}",
                board.board
            ))
            .await
            .check_not_found();
    }
    for page in [
        board.page.as_str(),
        board.card.as_str(),
        shared.gone.as_str(),
        UNKNOWN_ID,
    ] {
        anonymous
            .get(&shared.at(&format!("/pages/{page}")))
            .await
            .check_not_found();
    }
    th.teardown().await;
}

/// The link opens nothing once the wiki's sharing is off or its token made anew, the
/// organization's public links are off, or the wiki is in the trash.
#[tokio::test]
async fn a_wiki_link_opens_nothing_once_it_is_off() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let owner = &th.client;
    let anonymous = th.anonymous();
    let reads = |shared: &SharedWiki| [shared.at(""), shared.at("/pages")];
    // A new token.
    let shared = shared_wiki(&th).await;
    let sharing = format!("{TEST_WS}/containers/{}/public-link", shared.wiki);
    owner
        .post(&sharing, &json!({"enabled": true, "regenerate": true}))
        .await
        .check_ok();
    for path in reads(&shared) {
        anonymous.get(&path).await.check_not_found();
    }
    // PublicLink off.
    let shared = SharedWiki {
        token: th.share(&shared.wiki).await,
        ..shared
    };
    anonymous.get(&shared.at("")).await.check_ok();
    owner
        .post(&sharing, &json!({"enabled": false}))
        .await
        .check_ok();
    for path in reads(&shared) {
        anonymous.get(&path).await.check_not_found();
    }
    // The organization's public links off.
    let shared = SharedWiki {
        token: th.share(&shared.wiki).await,
        ..shared
    };
    anonymous.get(&shared.at("")).await.check_ok();
    let org = crate::org_admin::org_of(&th).await;
    owner
        .patch(&format!("/org/{org}"), &json!({"publicLinks": false}))
        .await
        .check_ok();
    for path in reads(&shared) {
        anonymous.get(&path).await.check_not_found();
    }
    owner
        .patch(&format!("/org/{org}"), &json!({"publicLinks": true}))
        .await
        .check_ok();
    anonymous.get(&shared.at("")).await.check_ok();
    // The wiki in the trash.
    owner
        .delete(&format!("{TEST_WS}/containers/{}", shared.wiki))
        .await
        .check_no_content();
    for path in reads(&shared) {
        anonymous.get(&path).await.check_not_found();
    }
    th.teardown().await;
}

/// A visit by a wiki's link is counted as anonymous on its roster, and the visitor is
/// sent nothing of who is there.
#[tokio::test]
async fn a_visit_by_a_wiki_link_is_counted_anonymously_and_told_nothing() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let shared = shared_wiki(&th).await;
    let mut member = Socket::connect_as(&th.client).await;
    visit(&mut member, &shared.wiki, "").await;
    let _ = people(&mut member, 1).await;
    let mut reader = Socket::connect(&th).await;
    reader
        .send(json!({"type": "setPresence", "containerId": shared.wiki,
            "cardId": shared.page, "readToken": shared.token}))
        .await;
    let roster = people(&mut member, 1).await;
    assert_eq!(roster["anonymousCount"], 1, "{roster}");
    assert!(
        reader.next_json(QUIET).await.is_none(),
        "a reader by the link hears nothing"
    );
    th.teardown().await;
}

/// The blocks a link follows over the websocket are a board's: a wiki's token follows
/// none of its pages, and is told nothing when one changes.
#[tokio::test]
async fn a_wiki_link_follows_no_blocks_over_the_websocket() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let shared = shared_wiki(&th).await;
    let mut reader = Socket::connect(&th).await;
    reader
        .send(json!({
            "type": "subscribeBlocks", "workspaceId": TEST_WORKSPACE_ID,
            "readToken": shared.token, "blockIds": [shared.page],
        }))
        .await;
    let path = format!("{TEST_WS}/wikis/{}/pages/{}", shared.wiki, shared.page);
    let read_at = th.client.get(&path).await.value()["updatedAt"].clone();
    th.client
        .patch(&path, &json!({"readAt": read_at, "title": "renamed"}))
        .await
        .check_ok();
    assert!(
        reader.next_json(QUIET).await.is_none(),
        "a wiki's token follows no page"
    );
    th.teardown().await;
}

/// A visit by a wiki's link ends when its token is made anew: it is counted no more.
#[tokio::test]
async fn a_visit_by_a_wiki_link_ends_with_its_token() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let shared = shared_wiki(&th).await;
    let mut member = Socket::connect_as(&th.client).await;
    visit(&mut member, &shared.wiki, "").await;
    let _ = people(&mut member, 1).await;
    let mut reader = Socket::connect(&th).await;
    reader
        .send(json!({"type": "setPresence", "containerId": shared.wiki,
            "cardId": shared.page, "readToken": shared.token}))
        .await;
    assert_eq!(people(&mut member, 1).await["anonymousCount"], 1);
    th.client
        .post(
            &format!("{TEST_WS}/containers/{}/public-link", shared.wiki),
            &json!({"enabled": true, "regenerate": true}),
        )
        .await
        .check_ok();
    assert_eq!(
        people(&mut member, 1).await["anonymousCount"],
        0,
        "the old token's visit ended"
    );
    th.teardown().await;
}
