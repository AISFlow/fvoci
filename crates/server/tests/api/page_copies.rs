//! Duplicating a wiki's page over HTTP (`duplicatePage`): the copy right after the page
//! with the pages below it, the images of its body given files of their own, told of,
//! for the wiki's editors, held to the organization's limits.

use fvoci_model::{Page, PageNode};
use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::files::{upload, uploaded_id};

async fn wiki(client: &Client) -> (String, String) {
    let made = client
        .post(
            &format!("{TEST_WS}/wikis"),
            &json!({"visibility": "private"}),
        )
        .await;
    made.check_created();
    let id = made.value()["id"].as_str().expect("an id").to_owned();
    (id.clone(), format!("{TEST_WS}/wikis/{id}"))
}

async fn make(client: &Client, wiki: &str, parent_id: &str, title: &str) -> Page {
    client
        .post(
            &format!("{wiki}/pages"),
            &json!({"parentId": parent_id, "title": title}),
        )
        .await
        .created_json()
}

/// The stored name of the first image a body shows.
fn image_of(body: &Value) -> Option<String> {
    body["content"][0]["attrs"]["fileId"]
        .as_str()
        .map(str::to_owned)
}

#[tokio::test]
async fn a_page_is_copied_with_those_below_it_its_images_given_files_and_told_of() {
    Box::pin(copied_with_those_below_it_and_given_files("image")).await;
}

#[tokio::test]
async fn a_page_is_copied_with_its_file_blocks_given_files() {
    Box::pin(copied_with_those_below_it_and_given_files("file")).await;
}

async fn copied_with_those_below_it_and_given_files(kind: &str) {
    let ts = Setup::native().start().await.init_basic().await;
    let client = &ts.client;
    let (wiki_id, wiki) = wiki(client).await;
    let a = make(client, &wiki, "", "Welcome").await;
    let b = make(client, &wiki, &a.id, "Getting started").await;
    let d = make(client, &wiki, "", "Policies").await;
    let file = uploaded_id(&upload(client, &wiki_id, "pic.png", b"a picture\n").await);
    let image = json!({"type": "doc", "content": [{"type": kind, "attrs": {"fileId": file}}]});
    client
        .patch(
            &format!("{wiki}/pages/{}", b.id),
            &json!({"body": image, "readAt": b.updated_at}),
        )
        .await
        .check_ok();

    client
        .patch(&format!("{wiki}/pages/{}", a.id), &json!({"icon": "📘"}))
        .await
        .check_ok();
    // Another editor of the wiki hears of the copies.
    let other = ts.client2.me().await.id;
    client
        .post(
            &format!("{TEST_WS}/containers/{wiki_id}/members"),
            &json!({"containerId": wiki_id, "userId": other, "role": "editor"}),
        )
        .await
        .check_ok();
    let mut socket = WorkspaceSocket::subscribe(&ts, &ts.client2, TEST_WORKSPACE_ID, &other).await;
    let copy: Page = client
        .post(
            &format!("{wiki}/pages/{}/duplicate", a.id),
            &json!({"title": "Welcome (copy)"}),
        )
        .await
        .created_json();
    assert_eq!(copy.title, "Welcome (copy)", "{kind}");
    assert_eq!(copy.icon, "📘", "its icon too");
    assert_ne!(copy.id, a.id, "{kind}");
    let tree: Vec<PageNode> = client.get(&format!("{wiki}/pages")).await.ok_json();
    let top: Vec<&str> = tree
        .iter()
        .filter(|page| page.parent_id.is_empty())
        .map(|page| page.title.as_str())
        .collect();
    assert_eq!(top, ["Welcome", "Welcome (copy)", "Policies"], "{kind}");
    let copied_b = tree
        .iter()
        .find(|page| page.parent_id == copy.id)
        .expect("a copy below the copy");
    assert_eq!(copied_b.title, "Getting started", "{kind}");
    let told = socket.expect("blockChanged", "/block/id", &copy.id).await;
    assert_eq!(told["block"]["title"], json!("Welcome (copy)"), "{kind}");
    socket
        .expect("blockChanged", "/block/id", &copied_b.id)
        .await;

    // The copy's image is a file of its own, which is there.
    let read: Page = client
        .get(&format!("{wiki}/pages/{}", copied_b.id))
        .await
        .ok_json();
    let body = serde_json::to_value(&read.body).expect("JSON");
    let copied_file = image_of(&body).expect("the copy shows an image");
    assert_ne!(copied_file, file, "{kind}");
    assert_eq!(
        body["content"][0]["type"],
        json!(kind),
        "still the same node"
    );
    client
        .get(&format!(
            "{TEST_WS}/containers/{wiki_id}/files/{copied_file}"
        ))
        .await
        .check_ok();
    // The copy begins without versions of its body (no route reads a page's yet), the
    // original keeps its own.
    let kept = |id: String| {
        let store = ts.app.store().clone();
        async move { store.body_versions(&id).await.expect("the versions").len() }
    };
    assert_eq!(kept(copied_b.id.clone()).await, 0, "{kind}");
    assert_ne!(kept(b.id.clone()).await, 0, "{kind}");
    // The original keeps its own.
    let original: Page = client
        .get(&format!("{wiki}/pages/{}", b.id))
        .await
        .ok_json();
    let body = serde_json::to_value(&original.body).expect("JSON");
    assert_eq!(image_of(&body), Some(file), "{kind}");
    assert_ne!(d.id, copy.id, "{kind}");
    ts.teardown().await;
}

#[tokio::test]
async fn a_copy_is_the_editors_held_to_the_limits_and_checked() {
    let ts = Setup::native()
        .configure(|config| config.wiki.wiki_pages = 3)
        .start()
        .await
        .init_basic()
        .await;
    let client = &ts.client;
    let (wiki_id, wiki) = wiki(client).await;
    let a = make(client, &wiki, "", "Welcome").await;
    make(client, &wiki, &a.id, "Getting started").await;
    let duplicate = format!("{wiki}/pages/{}/duplicate", a.id);

    let other = ts.client2.me().await.id;
    client
        .post(
            &format!("{TEST_WS}/containers/{wiki_id}/members"),
            &json!({"containerId": wiki_id, "userId": other, "role": "viewer"}),
        )
        .await
        .check_ok();
    ts.client2
        .post(&duplicate, &json!({}))
        .await
        .check_forbidden();

    // Two and two more would be four where three may be.
    let refused = client.post(&duplicate, &json!({})).await;
    refused.check_bad_request();
    assert_eq!(refused.value()["reason"], json!("too_many_pages"));
    assert_eq!(refused.value()["limit"], json!(3));
    let long = client
        .post(&duplicate, &json!({"title": "x".repeat(16_384)}))
        .await;
    long.check_bad_request();
    assert_eq!(
        long.value()["errors"][0]["pointer"],
        json!("/title"),
        "{}",
        long.value()
    );
    client
        .post(&duplicate, &json!({"parentId": ""}))
        .await
        .check_bad_request();
    let tree: Vec<PageNode> = client.get(&format!("{wiki}/pages")).await.ok_json();
    assert_eq!(tree.len(), 2, "nothing made");
    ts.teardown().await;
}
