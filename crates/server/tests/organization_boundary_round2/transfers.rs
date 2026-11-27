//! Subscriptions, templates, and an explicitly authorized archive transfer.

use base64::Engine as _;
use fvoci_model::{Container, ContainerMember, DuplicateAs, Subscription, User, new_id};
use fvoci_store::{BlockFilter, DbType};
use reqwest::Method;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt as _;

use super::common::ROOT_WS;
use super::common::{Client, ROOT_WORKSPACE_ID};
use super::db_type_of_the_tests;
use super::media::Scene;

pub(super) async fn import_bytes(client: &Client, workspace: &str, bytes: Vec<u8>) {
    let opened = client
        .post(
            &format!("/w/{workspace}/archive/uploads"),
            &json!({"name": "transfer.zip", "size": bytes.len()}),
        )
        .await;
    opened.check_created();
    let upload = opened.value()["uploadId"]
        .as_str()
        .expect("an upload ID")
        .to_owned();
    let digest = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(&bytes));
    Client::send(
        client
            .request(Method::PUT, &format!("{ROOT_WS}/uploads/{upload}/parts/1"))
            .header("Content-Digest", format!("sha-256=:{digest}:"))
            .body(bytes),
    )
    .await
    .check_no_content();
    client
        .post(
            &format!("/w/{workspace}/archive/import"),
            &json!({"uploadId": upload}),
        )
        .await
        .check_ok();
}

#[tokio::test]
async fn subscriptions_are_scoped_even_when_the_subscriber_uses_both_organizations() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let scene = Scene::new(None).await;
    for (app, card) in [
        (&scene.ts.app, &scene.card_a),
        (&scene.b_app, &scene.card_b),
    ] {
        app.create_subscription(Subscription {
            block_type: card.block_type,
            block_id: card.id.clone(),
            user_id: scene.user.clone(),
            notified_at: 0,
            created_at: 0,
            deleted_at: None,
        })
        .await
        .expect("the subscriber has access in this organization");
    }
    let mine: Vec<Subscription> = scene
        .ts
        .client
        .get(&format!("{ROOT_WS}/subscriptions/{}", scene.user))
        .await
        .ok_json();
    assert_eq!(
        mine.iter()
            .map(|sub| sub.block_id.as_str())
            .collect::<Vec<_>>(),
        [scene.card_a.id.as_str()],
        "the old HTTP route reads only A"
    );
    scene
        .ts
        .client
        .delete(&format!(
            "{ROOT_WS}/subscriptions/{}/{}",
            scene.card_b.id, scene.user
        ))
        .await
        .check_not_found();
    let b = scene
        .b_app
        .subscriptions(&scene.user)
        .await
        .expect("B's subscription remains");
    assert_eq!(
        b[0].block_id, scene.card_b.id,
        "A's deletion did not reach B"
    );
    scene
        .ts
        .client2
        .get(&format!("{ROOT_WS}/subscriptions/{}", scene.user))
        .await
        .check_not_found();
    let hidden = scene
        .ts
        .client2
        .post(
            &format!("{ROOT_WS}/subscriptions"),
            &json!({"blockId": scene.card_b.id, "blockType": "card", "userId": scene.ts.client2.me().await.id}),
        )
        .await;
    hidden.check_not_found();
    scene.ts.teardown().await;
}

async fn export_with_b_only_member(scene: &Scene) -> (Vec<u8>, User) {
    let mut outsider = User {
        id: new_id(),
        username: "B-only-member".to_owned(),
        email: "b-only@example.com".to_owned(),
        ..User::default()
    };
    scene
        .b_app
        .store()
        .create_user(&mut outsider)
        .await
        .expect("a B-only account");
    scene
        .b_app
        .store()
        .save_member(&ContainerMember {
            container_id: scene.b.id.clone(),
            user_id: outsider.id.clone(),
            role: fvoci_model::ContainerRole::Viewer,
            synthetic: false,
            inherited_role: None,
        })
        .await
        .expect("B's invited member");
    assert!(
        scene
            .ts
            .app
            .export_archive(ROOT_WORKSPACE_ID, std::slice::from_ref(&scene.b.id))
            .await
            .is_err(),
        "A's service cannot export B's board"
    );
    let mut archive = scene
        .b_app
        .export_archive(&scene.b.workspace_id, std::slice::from_ref(&scene.b.id))
        .await
        .expect("the user explicitly exports B through B's service");
    let mut bytes = Vec::new();
    archive
        .file
        .read_to_end(&mut bytes)
        .await
        .expect("the exported bytes");
    drop(archive);
    (bytes, outsider)
}

#[tokio::test]
async fn an_authorized_archive_transfer_remaps_ids_files_and_members_into_the_destination_org() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let scene = Scene::new(None).await;
    let (_, name_b) = scene.receive_files().await;
    let (bytes, outsider) = export_with_b_only_member(&scene).await;
    import_bytes(&scene.ts.client, ROOT_WORKSPACE_ID, bytes).await;
    let boards: Vec<Container> = scene
        .ts
        .client
        .get(&format!("{ROOT_WS}/boards"))
        .await
        .ok_json();
    let imported = boards
        .iter()
        .find(|board| board.title == scene.b.title)
        .expect("B was explicitly copied into A");
    assert_ne!(
        imported.id, scene.b.id,
        "the original identifier is not reused"
    );
    assert_eq!(
        imported.workspace_id, ROOT_WORKSPACE_ID,
        "the target workspace wins over archive metadata"
    );
    let members: Vec<ContainerMember> = scene
        .ts
        .client
        .get(&format!("{ROOT_WS}/containers/{}/members", imported.id))
        .await
        .ok_json();
    assert!(
        !members.iter().any(|member| member.user_id == outsider.id),
        "a B-only member is not admitted to A"
    );
    let blocks = scene
        .ts
        .app
        .store()
        .get_blocks(&BlockFilter::container(&imported.id))
        .await
        .expect("the imported blocks");
    let imported_name = blocks
        .iter()
        .find_map(|block| block.fields.get("fileId").and_then(|value| value.as_str()))
        .expect("the imported attachment");
    assert_ne!(imported_name, name_b, "the file receives its own key");
    let file = scene
        .ts
        .client
        .get(&format!(
            "{ROOT_WS}/containers/{}/files/{imported_name}",
            imported.id
        ))
        .await;
    file.check_ok();
    assert_eq!(
        file.body, b"organization B file",
        "the explicitly imported bytes survive"
    );
    let original = scene
        .b_app
        .store()
        .get_file_record(&scene.b.id, &name_b)
        .await
        .expect("the original is still B's");
    assert!(
        scene
            .b_app
            .files()
            .exists(&original.path)
            .await
            .expect("B's original object"),
        "import did not move or erase B's object"
    );
    scene.ts.teardown().await;
}

#[tokio::test]
async fn system_templates_are_shared_read_only_and_their_copies_belong_to_the_destination() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let scene = Scene::new(None).await;
    scene
        .ts
        .app
        .init_templates()
        .await
        .expect("install the system templates");
    let templates = scene
        .ts
        .app
        .store()
        .get_builtin_templates()
        .await
        .expect("A's templates");
    let b_templates = scene
        .b_app
        .store()
        .get_builtin_templates()
        .await
        .expect("B's templates");
    assert!(
        !templates.is_empty(),
        "the test exercises installed templates"
    );
    assert_eq!(
        templates, b_templates,
        "both organizations can read the system's templates"
    );
    let builtin = &templates[0];
    let changed = scene
        .b_app
        .store()
        .execute_unchecked(&format!(
            "UPDATE containers SET title = 'Changed by B' WHERE id = '{}'",
            builtin.id
        ))
        .await
        .expect("the policy applies");
    assert_eq!(changed, 0, "B has no write policy on the system's template");
    let (copied, _) = scene
        .b_app
        .duplicate_container(
            &builtin.id,
            &scene.user,
            &scene.b.workspace_id,
            DuplicateAs::Copy,
            None,
        )
        .await
        .expect("an authorized template copy into B");
    let board = &copied.containers[0];
    assert!(
        !board.is_builtin && board.workspace_id == scene.b.workspace_id,
        "the copy is B's regular board"
    );
    assert!(
        scene.ts.app.store().get_container(&board.id).await.is_err(),
        "A cannot read the new B row"
    );
    scene.ts.teardown().await;
}

#[tokio::test]
async fn one_user_can_file_a_shared_builtin_template_independently_in_two_orgs() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let scene = Scene::new(None).await;
    scene
        .ts
        .app
        .init_templates()
        .await
        .expect("system templates installed");
    let builtin = scene
        .ts
        .app
        .store()
        .get_builtin_templates()
        .await
        .expect("the shared templates")
        .remove(0);
    let a_categories = scene
        .ts
        .app
        .user_category_containers(&scene.user, ROOT_WORKSPACE_ID)
        .await
        .expect("A's categories");
    let b_categories = scene
        .b_app
        .user_category_containers(&scene.user, &scene.b.workspace_id)
        .await
        .expect("B's categories");
    scene
        .ts
        .app
        .add_update_user_category_container(
            ROOT_WORKSPACE_ID,
            &scene.user,
            &a_categories[0].category.id,
            std::slice::from_ref(&builtin.id),
        )
        .await
        .expect("the template is filed in A");
    let b_filed = scene
        .b_app
        .add_update_user_category_container(
            &scene.b.workspace_id,
            &scene.user,
            &b_categories[0].category.id,
            std::slice::from_ref(&builtin.id),
        )
        .await;
    scene.ts.teardown().await;
    assert!(
        b_filed.is_ok(),
        "A's category row prevented B's independent filing of the shared system template: {b_filed:?}"
    );
}
