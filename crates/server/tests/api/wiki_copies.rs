//! Copying a wiki whole (`duplicateContainer` of a wiki): its pages as they stand, their
//! tree and bodies, the images given files of their own, and what a body names among
//! the wiki's pages named by the copies; not its comments, versions, trash, members or
//! public link. A wiki is no template, and the copy is held to the limits.

use std::collections::HashMap;

use fvoci_model::{
    ContainerKind, ContainerMember, ContainerVisibility, ContainersAndBlocks, NewPage, PageLimits,
    PageNode,
};
use serde_json::{Value, json};

use crate::common::{Client, Setup, TEST_WS, TestServer};
use crate::files::{upload, uploaded_id};
use crate::references::made;
use crate::references_shared::references_in;
use crate::references_to::body_naming;

/// A wiki of the client's in the workspace at `TEST_WS`: its identifier.
pub(crate) async fn wiki(client: &Client, title: &str) -> String {
    let made = client
        .post(
            &format!("{TEST_WS}/wikis"),
            &json!({"visibility": "open", "title": title, "description": "how we work", "icon": "📗"}),
        )
        .await;
    made.check_created();
    made.value()["id"].as_str().expect("an id").to_owned()
}

/// A page of the wiki below `parent_id`: its identifier.
pub(crate) async fn page(client: &Client, wiki: &str, parent_id: &str, title: &str) -> String {
    let made = client
        .post(
            &format!("{TEST_WS}/wikis/{wiki}/pages"),
            &json!({"parentId": parent_id, "title": title}),
        )
        .await;
    made.check_created();
    made.value()["id"].as_str().expect("an id").to_owned()
}

/// Writes the body to the page.
pub(crate) async fn write(client: &Client, wiki: &str, page: &str, body: &Value) {
    let path = format!("{TEST_WS}/wikis/{wiki}/pages/{page}");
    let read_at = client.get(&path).await.value()["updatedAt"].clone();
    client
        .patch(&path, &json!({"readAt": read_at, "body": body}))
        .await
        .check_ok();
}

/// The body of the page of the wiki of the workspace at `at`.
pub(crate) async fn body_of(client: &Client, at: &str, wiki: &str, page: &str) -> Value {
    let read = client.get(&format!("{at}/wikis/{wiki}/pages/{page}")).await;
    read.check_ok();
    read.value()["body"].clone()
}

/// The pages of the wiki of the workspace at `at` as its tree lists them.
pub(crate) async fn tree_of(client: &Client, at: &str, wiki: &str) -> Vec<PageNode> {
    client
        .get(&format!("{at}/wikis/{wiki}/pages"))
        .await
        .ok_json()
}

/// The tree as titles: each page's title and its parent's, in the order listed.
pub(crate) fn titled(tree: &[PageNode]) -> Vec<(String, String)> {
    let titles: HashMap<&str, &str> = tree
        .iter()
        .map(|page| (page.id.as_str(), page.title.as_str()))
        .collect();
    tree.iter()
        .map(|page| {
            let parent = titles.get(page.parent_id.as_str()).copied().unwrap_or("");
            (page.title.clone(), parent.to_owned())
        })
        .collect()
}

/// The page of the tree titled `title`: its identifier.
pub(crate) fn titled_id(tree: &[PageNode], title: &str) -> String {
    tree.iter()
        .find(|page| page.title == title)
        .unwrap_or_else(|| panic!("a page titled {title}"))
        .id
        .clone()
}

/// Copies the board or wiki as the client: the answer.
async fn duplicate(client: &Client, board_id: &str, query: &str) -> crate::common::ApiResponse {
    client
        .post(
            &format!("{TEST_WS}/containers/{board_id}/duplicate{query}"),
            &json!({}),
        )
        .await
}

#[tokio::test]
async fn a_wiki_is_copied_with_its_pages_bodies_and_images_and_names_its_own_copies() {
    wiki_copied_with_its_files("image").await;
}

#[tokio::test]
async fn a_wiki_is_copied_with_its_file_blocks_and_names_its_own_copies() {
    wiki_copied_with_its_files("file").await;
}

#[expect(
    clippy::too_many_lines,
    reason = "the pages, their bodies and their images, checked on one copy"
)]
async fn wiki_copied_with_its_files(kind: &str) {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let client = &ts.client;
    let elsewhere = made(client, TEST_WS, "open", "elsewhere").await;
    let wiki = wiki(client, "Handbook").await;
    let a = page(client, &wiki, "", "Welcome").await;
    let b = page(client, &wiki, &a, "Getting started").await;
    page(client, &wiki, &a, "Second steps").await;
    page(client, &wiki, "", "Policies").await;
    let old = page(client, &wiki, "", "Old").await;
    client
        .delete(&format!("{TEST_WS}/wikis/{wiki}/pages/{old}"))
        .await
        .check_no_content();
    let file = uploaded_id(&upload(client, &wiki, "pic.png", b"a picture\n").await);
    let image = json!({"type": "doc", "content": [{"type": kind, "attrs": {"fileId": file}}]});
    write(client, &wiki, &b, &image).await;
    let naming = body_naming("see", &[&b, &elsewhere.card, &elsewhere.page, &a]);
    write(client, &wiki, &a, &naming).await;
    client
        .post(
            &format!("{TEST_WS}/wikis/{wiki}/pages/{a}/comments"),
            &json!({"body": body_naming("a comment", &[])}),
        )
        .await
        .check_created();
    client
        .post(
            &format!("{TEST_WS}/containers/{wiki}/public-link"),
            &json!({"enabled": true}),
        )
        .await
        .check_ok();
    let other = ts.client2.me().await.id;
    client
        .post(
            &format!("{TEST_WS}/containers/{wiki}/members"),
            &json!({"containerId": wiki, "userId": other, "role": "editor"}),
        )
        .await
        .check_ok();

    let copies: ContainersAndBlocks = duplicate(client, &wiki, "").await.created_json();
    let copy = &copies.containers[0];
    assert_eq!(copy.kind, ContainerKind::Wiki, "{kind}");
    assert_eq!(copy.title, "Handbook copy", "{kind}");
    assert_eq!(
        (copy.description.as_str(), copy.icon.as_str()),
        ("how we work", "📗"),
        "{kind}"
    );
    assert_eq!(
        copy.visibility,
        ContainerVisibility::Private,
        "a copy is private"
    );
    assert_ne!(copy.id, wiki, "{kind}");

    // The same tree, in the same order, without the page in the trash.
    let source_tree = tree_of(client, TEST_WS, &wiki).await;
    let copy_tree = tree_of(client, TEST_WS, &copy.id).await;
    assert_eq!(titled(&copy_tree), titled(&source_tree), "{kind}");
    assert!(
        !titled(&copy_tree).iter().any(|(title, _)| title == "Old"),
        "{kind}"
    );
    assert!(
        copy_tree.iter().all(|page| page.id != a && page.id != b),
        "every page is a copy"
    );
    let (copy_a, copy_b) = (
        titled_id(&copy_tree, "Welcome"),
        titled_id(&copy_tree, "Getting started"),
    );

    // What the body named among the wiki's pages, it names by the copies; the rest stays.
    let named = references_in(&body_of(client, TEST_WS, &copy.id, &copy_a).await);
    assert_eq!(
        named,
        [
            copy_b.clone(),
            elsewhere.card.clone(),
            elsewhere.page.clone(),
            copy_a.clone()
        ],
        "{kind}"
    );
    let naming_copy_b: Vec<String> = client
        .get(&format!("{TEST_WS}/references/to/{copy_b}"))
        .await
        .ok_json::<Vec<Value>>()
        .iter()
        .map(|linked| linked["id"].as_str().expect("an id").to_owned())
        .collect();
    assert_eq!(
        naming_copy_b,
        std::slice::from_ref(&copy_a),
        "counted for the copies"
    );

    // The image is a file of the copy's own, which is there.
    let body = body_of(client, TEST_WS, &copy.id, &copy_b).await;
    let copied_file = body["content"][0]["attrs"]["fileId"]
        .as_str()
        .expect("the copy shows an image")
        .to_owned();
    assert_ne!(copied_file, file, "{kind}");
    assert_eq!(
        body["content"][0]["type"],
        json!(kind),
        "still the same node"
    );
    let read = client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{copied_file}",
            copy.id
        ))
        .await;
    read.check_ok();
    assert_eq!(read.body, b"a picture\n", "{kind}");

    check_left_behind(&ts, &copy.id, &copy_a).await;
    ts.teardown().await;
}

/// Checks that the copy has no comments, versions, trash, other members or public link.
async fn check_left_behind(ts: &TestServer, copy: &str, copy_a: &str) {
    let client = &ts.client;
    let comments: Vec<Value> = client
        .get(&format!("{TEST_WS}/wikis/{copy}/pages/{copy_a}/comments"))
        .await
        .ok_json();
    assert!(comments.is_empty(), "no comment is copied");
    let store = ts.app.store();
    for page in tree_of(client, TEST_WS, copy).await {
        let versions = store.body_versions(&page.id).await.expect("the versions");
        assert!(versions.is_empty(), "the copy begins without versions");
    }
    let trash = store
        .get_deleted_pages(copy, None)
        .await
        .expect("the trash");
    assert!(trash.is_empty(), "nothing of the trash is copied");
    let members: Vec<ContainerMember> = client
        .get(&format!("{TEST_WS}/containers/{copy}/members"))
        .await
        .ok_json();
    let me = client.me().await.id;
    assert_eq!(members.len(), 1, "who copies it alone");
    assert!(
        members[0].user_id == me && members[0].role == fvoci_model::ContainerRole::Admin,
        "who copies it administers the copy"
    );
    let sharing = client
        .get(&format!("{TEST_WS}/containers/{copy}/public-link"))
        .await;
    sharing.check_ok();
    assert_eq!(sharing.value()["enabled"], json!(false), "no public link");
}

#[tokio::test]
async fn a_wiki_is_no_template_and_who_does_not_read_it_finds_none() {
    let ts = Setup::native().start().await.init_basic().await;
    let client = &ts.client;
    let wiki = wiki(client, "Handbook").await;
    page(client, &wiki, "", "Welcome").await;
    let refused = duplicate(client, &wiki, "?asTemplate=true").await;
    refused.check_bad_request();
    assert_eq!(refused.refused_at(), "parameter asTemplate");
    let private = client
        .post(
            &format!("{TEST_WS}/wikis"),
            &json!({"visibility": "private", "title": "Private"}),
        )
        .await;
    private.check_created();
    let private = private.value()["id"].as_str().expect("an id").to_owned();
    duplicate(&ts.client2, &private, "").await.check_not_found();
    let wikis: Vec<Value> = client.get(&format!("{TEST_WS}/wikis")).await.ok_json();
    assert_eq!(wikis.len(), 2, "no copy was made");
    ts.teardown().await;
}

#[tokio::test]
async fn a_copy_is_held_to_the_limits_lowered_since_the_wiki_grew() {
    let ts = Setup::native()
        .configure(|config| {
            config.wiki.wiki_pages = 3;
            config.wiki.wiki_page_depth = 2;
        })
        .start()
        .await
        .init_basic()
        .await;
    let client = &ts.client;
    let me = client.me().await.id;
    let roomy = PageLimits {
        depth: 10,
        pages: 10,
    };
    let store = ts.app.store();
    let grow = |wiki: String, parent: String, title: &'static str| {
        let me = me.clone();
        async move {
            let new = NewPage {
                title: title.to_owned(),
                parent_id: parent,
                ..NewPage::default()
            };
            store
                .create_page(&wiki, &new, roomy, &me)
                .await
                .expect("a page past the limits of the server")
                .first
                .id
        }
    };

    let wide = wiki(client, "Wide").await;
    for title in ["one", "two", "three", "four"] {
        grow(wide.clone(), String::new(), title).await;
    }
    let refused = duplicate(client, &wide, "").await;
    refused.check_bad_request();
    let problem = refused.check_problem();
    assert_eq!(problem["reason"], json!("too_many_pages"));
    assert_eq!(problem["limit"], json!(3));

    let deep = wiki(client, "Deep").await;
    let top = grow(deep.clone(), String::new(), "top").await;
    let middle = grow(deep.clone(), top, "middle").await;
    grow(deep.clone(), middle, "bottom").await;
    let refused = duplicate(client, &deep, "").await;
    refused.check_bad_request();
    let problem = refused.check_problem();
    assert_eq!(problem["reason"], json!("page_too_deep"));
    assert_eq!(problem["limit"], json!(2));

    // Within the limits, it is copied.
    let small = wiki(client, "Small").await;
    let top = page(client, &small, "", "top").await;
    page(client, &small, &top, "below").await;
    duplicate(client, &small, "").await.check_created();
    let wikis: Vec<Value> = client.get(&format!("{TEST_WS}/wikis")).await.ok_json();
    assert_eq!(wikis.len(), 4, "the two refused made nothing");
    ts.teardown().await;
}

#[tokio::test]
async fn a_wiki_is_copied_into_another_workspace_of_its_copier_with_its_files() {
    wiki_copied_into_another_workspace("image").await;
}

#[tokio::test]
async fn a_wiki_is_copied_into_another_workspace_with_the_files_of_its_file_blocks() {
    wiki_copied_into_another_workspace("file").await;
}

async fn wiki_copied_into_another_workspace(kind: &str) {
    let ts = Setup::native().start().await.init_basic().await;
    let client = &ts.client;
    let wiki = wiki(client, "Handbook").await;
    let a = page(client, &wiki, "", "Welcome").await;
    let file = uploaded_id(&upload(client, &wiki, "pic.png", b"a picture\n").await);
    let image = json!({"type": "doc", "content": [{"type": kind, "attrs": {"fileId": file}}]});
    write(client, &wiki, &a, &image).await;
    let me = client.me().await.id;
    let other = crate::boards::another_workspace(&ts, &me).await;
    let copies: ContainersAndBlocks = duplicate(client, &wiki, &format!("?toWorkspace={other}"))
        .await
        .created_json();
    let copy = &copies.containers[0];
    assert_eq!(copy.workspace_id, other, "{kind}");
    let at = format!("/w/{other}");
    let copy_a = titled_id(&tree_of(client, &at, &copy.id).await, "Welcome");
    let body = body_of(client, &at, &copy.id, &copy_a).await;
    let copied_file = body["content"][0]["attrs"]["fileId"]
        .as_str()
        .expect("the copy shows the picture")
        .to_owned();
    assert_ne!(copied_file, file, "{kind}");
    assert_eq!(
        body["content"][0]["type"],
        json!(kind),
        "still the same node"
    );
    let read = client
        .get(&format!("{at}/containers/{}/files/{copied_file}", copy.id))
        .await;
    read.check_ok();
    assert_eq!(read.body, b"a picture\n", "{kind}");
    // Not into a workspace the copier is not in.
    let stranger = crate::boards::another_workspace(&ts, &ts.client2.me().await.id).await;
    duplicate(client, &wiki, &format!("?toWorkspace={stranger}"))
        .await
        .check_not_found();
    ts.teardown().await;
}
