//! A wiki of another organization: copied whole only by its own organization's service,
//! never into a workspace of another; exported only by it, and imported elsewhere under
//! identifiers of its own, what it names of its organization nothing there.

use fvoci_model::{
    Body, Container, ContainerKind, ContainerKinds, ContainerVisibility, NewPage, PageLimits,
    PageNode, new_id,
};
use fvoci_model::{DuplicateAs, PublicContainers};
use fvoci_store::{BlockFilter, DbType};

use serde_json::json;
use tokio::io::AsyncReadExt as _;

use super::common::{ROOT_WORKSPACE_ID, ROOT_WS};
use super::db_type_of_the_tests;
use super::media::Scene;
use super::transfers::import_bytes;

/// A wiki of organization B with a page: the wiki and the page.
pub(super) async fn b_wiki(scene: &Scene) -> (Container, String) {
    let mut wiki = Container {
        id: new_id(),
        workspace_id: scene.b.workspace_id.clone(),
        visibility: ContainerVisibility::Private,
        kind: ContainerKind::Wiki,
        title: "B's handbook".to_owned(),
        ..Container::default()
    };
    let store = scene.b_app.store();
    store
        .insert_container_with_admin(&mut wiki, &scene.user)
        .await
        .expect("B's wiki");
    let new = NewPage {
        title: "B's page".to_owned(),
        ..NewPage::default()
    };
    let page = store
        .create_page(&wiki.id, &new, PageLimits::default(), &scene.user)
        .await
        .expect("B's page")
        .page
        .id;
    (wiki, page)
}

#[tokio::test]
async fn a_wiki_is_copied_by_its_own_organization_and_into_none_other() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let scene = Scene::new(None).await;
    let (wiki, page) = b_wiki(&scene).await;
    let boards_now = async || {
        let of = async |app: &fvoci_server::app::App, workspace: &str| {
            app.store()
                .get_containers_for_user_and_workspace(
                    &scene.user,
                    workspace,
                    PublicContainers::Include,
                    ContainerKinds::EVERY,
                )
                .await
                .expect("the boards")
                .len()
        };
        (
            of(&scene.ts.app, ROOT_WORKSPACE_ID).await,
            of(&scene.b_app, &scene.b.workspace_id).await,
        )
    };
    let before = boards_now().await;
    let from_a = scene
        .ts
        .app
        .duplicate_container(
            &wiki.id,
            &scene.user,
            ROOT_WORKSPACE_ID,
            DuplicateAs::Copy,
            None,
        )
        .await
        .expect_err("A's service does not find B's wiki");
    assert!(from_a.is_not_found(), "{from_a}");
    let into_a = scene
        .b_app
        .duplicate_container(
            &wiki.id,
            &scene.user,
            ROOT_WORKSPACE_ID,
            DuplicateAs::Copy,
            None,
        )
        .await
        .expect_err("B's service does not copy into A's workspace");
    assert!(into_a.is_not_found(), "{into_a}");
    assert_eq!(boards_now().await, before, "nothing is written in either");
    let (copies, _) = scene
        .b_app
        .duplicate_container(
            &wiki.id,
            &scene.user,
            &scene.b.workspace_id,
            DuplicateAs::Copy,
            None,
        )
        .await
        .expect("B copies its own wiki");
    let copy = &copies.containers[0];
    assert_eq!(copy.workspace_id, scene.b.workspace_id);
    assert!(
        scene.ts.app.store().get_container(&copy.id).await.is_err(),
        "A cannot read B's copy"
    );
    let pages = scene
        .b_app
        .store()
        .get_blocks(&BlockFilter::container(&copy.id))
        .await
        .expect("the copy's pages");
    assert_eq!(pages.len(), 1);
    assert_ne!(pages[0].id, page, "the copy's own page");
    scene.ts.teardown().await;
}

#[tokio::test]
async fn a_wiki_of_another_organization_is_imported_with_ids_of_its_own_naming_nothing_there() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let scene = Scene::new(None).await;
    let (wiki, page) = b_wiki(&scene).await;
    // B's page names B's card, which is no block of the archive.
    let naming = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "reference", "attrs": {"id": scene.card_b.id}}
    ]}]});
    let body = Body::parse(naming).expect("a body");
    scene
        .b_app
        .store()
        .replace_bodies_as_written(&wiki.id, vec![(page.clone(), body)])
        .await
        .expect("B's page names B's card");
    assert!(
        scene
            .ts
            .app
            .export_archive(ROOT_WORKSPACE_ID, std::slice::from_ref(&wiki.id))
            .await
            .is_err(),
        "A's service cannot export B's wiki"
    );
    let mut archive = scene
        .b_app
        .export_archive(&scene.b.workspace_id, std::slice::from_ref(&wiki.id))
        .await
        .expect("B's service exports B's wiki for its user");
    let mut bytes = Vec::new();
    archive
        .file
        .read_to_end(&mut bytes)
        .await
        .expect("the bytes");
    drop(archive);
    import_bytes(&scene.ts.client, ROOT_WORKSPACE_ID, bytes).await;

    let client = &scene.ts.client;
    let wikis: Vec<Container> = client.get(&format!("{ROOT_WS}/wikis")).await.ok_json();
    let imported = wikis
        .iter()
        .find(|one| one.title == wiki.title)
        .expect("B's wiki is copied into A");
    assert_ne!(imported.id, wiki.id);
    assert_eq!(imported.workspace_id, ROOT_WORKSPACE_ID);
    let pages: Vec<PageNode> = client
        .get(&format!("{ROOT_WS}/wikis/{}/pages", imported.id))
        .await
        .ok_json();
    assert_eq!(pages.len(), 1);
    assert_ne!(pages[0].id, page, "the page has an identifier of its own");
    let read = client
        .get(&format!(
            "{ROOT_WS}/wikis/{}/pages/{}",
            imported.id, pages[0].id
        ))
        .await;
    let named = read.ok_json::<serde_json::Value>()["body"].to_string();
    assert!(named.contains(&scene.card_b.id), "the body is as it was");
    let resolved = client
        .post(
            &format!("{ROOT_WS}/references/resolve"),
            &[scene.card_b.id.as_str()],
        )
        .await;
    assert_eq!(
        resolved.ok_json::<serde_json::Value>(),
        json!([]),
        "B's card is nothing in A"
    );
    scene.ts.teardown().await;
}
