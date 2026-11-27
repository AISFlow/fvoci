//! Files, read tokens, partial uploads and housekeeping across organizations.

use bytes::Bytes;
use fvoci_model::PublicLinkChange;
use fvoci_model::{
    Block, BlockType, Container, ContainerVisibility, Upload, Workspace, WorkspaceRole, new_id,
};
use fvoci_server::app::{App, FileToUpload};
use fvoci_server::config::{Configuration, S3Config};
use fvoci_store::{DbType, Tenant};
use reqwest::{Method, StatusCode};
use serde_json::json;
use sha2::{Digest as _, Sha256};

use super::common::ROOT_WS;
use super::common::{Client, ROOT_WORKSPACE_ID, Setup};
use super::{answer, db_type_of_the_tests, open};

async fn upload(app: &App, user: &str, board: &str, bytes: &[u8]) -> Upload {
    let upload = app
        .create_upload(
            user,
            board,
            FileToUpload {
                name: "private.bin".to_owned(),
                size: i64::try_from(bytes.len()).expect("small data"),
                content_type: "application/octet-stream".to_owned(),
            },
        )
        .await
        .expect("the upload opens in its organization");
    let digest = Sha256::digest(bytes).into();
    app.put_upload_part(
        &upload,
        1,
        u64::try_from(bytes.len()).expect("small data"),
        &digest,
        futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::copy_from_slice(bytes))]),
    )
    .await
    .expect("the partial upload receives its bytes");
    upload
}

async fn attachment(app: &App, user: &str, board: &str, card: &str, name: &str) {
    let mut block = Block {
        id: new_id(),
        container_id: board.to_owned(),
        parent_id: card.to_owned(),
        block_type: BlockType::Attachment,
        fields: json!({"fileId": name}).as_object().expect("fields").clone(),
        ..Block::default()
    };
    app.store()
        .insert_block(&mut block, user)
        .await
        .expect("the file is referenced");
}

pub(super) struct Scene {
    pub(super) ts: super::common::TestServer,
    pub(super) b_app: App,
    /// The second organization, of `b_app`.
    b_org: String,
    pub(super) user: String,
    pub(super) a: Container,
    pub(super) b: Container,
    other: Container,
    pub(super) card_a: Block,
    pub(super) card_b: Block,
    card_other: Block,
}

impl Scene {
    pub(super) async fn new(configure: Option<fn(&mut Configuration)>) -> Self {
        let setup = Setup::local_mode().configure(open);
        let setup = configure.map_or(setup, |configure| {
            Setup::local_mode().configure(open).configure(configure)
        });
        let ts = setup.start().await.init_basic().await;
        let user = ts.client.me().await.id;
        let a = ts
            .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private)
            .await;
        let workspace = Workspace {
            id: new_id(),
            ..Workspace::default()
        };
        let org = ts
            .app
            .store()
            .create_organization("Media organization", &workspace)
            .await
            .expect("the second organization");
        let b_app = ts
            .app
            .scoped(Tenant::org(&org).expect("an organization ID"));
        b_app
            .store()
            .add_workspace_member(&workspace.id, &user, WorkspaceRole::Member)
            .await
            .expect("the same user is admitted to the other workspace");
        let mut b = Container {
            id: new_id(),
            workspace_id: workspace.id.clone(),
            visibility: ContainerVisibility::Private,
            title: "B only".to_owned(),
            ..Container::default()
        };
        b_app
            .store()
            .insert_container_with_admin(&mut b, &user)
            .await
            .expect("the second organization's board");
        let mut other = Container {
            id: new_id(),
            workspace_id: workspace.id,
            visibility: ContainerVisibility::Private,
            ..Container::default()
        };
        b_app
            .store()
            .insert_container_with_admin(&mut other, &user)
            .await
            .expect("a different board of the second organization");

        let card_a = card(&ts.app, &user, &a.id).await;
        let card_b = card(&b_app, &user, &b.id).await;
        let card_other = card(&b_app, &user, &other.id).await;
        Self {
            ts,
            b_app,
            b_org: org,
            user,
            a,
            b,
            other,
            card_a,
            card_b,
            card_other,
        }
    }
    pub(super) async fn receive_files(&self) -> (String, String) {
        let Self {
            ts, b_app, a, b, ..
        } = self;
        let user = self.user.as_str();
        let bytes_a = b"organization A file";
        let bytes_b = b"organization B file";
        let ua = upload(&ts.app, user, &a.id, bytes_a).await;
        let ub = upload(b_app, user, &b.id, bytes_b).await;
        assert!(
            ts.app.upload(&ub.id, user).await.is_err(),
            "same user, wrong organization cannot resume B's upload"
        );
        assert!(
            b_app.upload(&ua.id, user).await.is_err(),
            "B cannot resume A's upload"
        );
        // Public HTTP upload-ID operations must hide B's upload even from its owner when
        // the old route is still using A. In particular they must not erase B's parts.
        for method in [Method::GET, Method::DELETE, Method::POST, Method::PUT] {
            let suffix = match method {
                Method::POST => "/complete",
                Method::PUT => "/parts/1",
                _ => "",
            };
            let hidden = Client::send(
                ts.client
                    .request(
                        method.clone(),
                        &format!("{ROOT_WS}/uploads/{}{suffix}", ub.id),
                    )
                    .body("x"),
            )
            .await;
            let absent = Client::send(
                ts.client
                    .request(
                        method.clone(),
                        &format!("{ROOT_WS}/uploads/missing-upload{suffix}"),
                    )
                    .body("x"),
            )
            .await;
            assert_eq!(
                answer(&hidden),
                answer(&absent),
                "{method} must not expose an upload of another organization"
            );
        }
        let name_a = ts
            .app
            .complete_upload(&ua)
            .await
            .expect("A's file is completed");
        let name_b = b_app
            .complete_upload(&ub)
            .await
            .expect("B's untouched parts are completed");
        attachment(&ts.app, user, &a.id, &self.card_a.id, &name_a).await;
        attachment(b_app, user, &b.id, &self.card_b.id, &name_b).await;
        (name_a, name_b)
    }
    /// The key of each organization's file begins with that organization and its board,
    /// and its object is there, whichever storage holds it.
    async fn check_keys(&self, name_a: &str, name_b: &str) {
        let Self {
            ts, b_app, a, b, ..
        } = self;
        let org_a = ts
            .app
            .store()
            .org_of_container(&a.id)
            .await
            .expect("A's organization");
        assert_ne!(org_a, self.b_org, "two organizations");
        for (app, org, board, name) in [
            (&ts.app, org_a.as_str(), &a.id, name_a),
            (b_app, self.b_org.as_str(), &b.id, name_b),
        ] {
            let info = app
                .store()
                .get_file_record(board, name)
                .await
                .expect("the file's record");
            assert!(
                info.path.starts_with(&format!("{org}/{board}/")),
                "{} begins with its organization and board",
                info.path
            );
            assert!(
                !info.path.contains('.'),
                "no extension in the key {}",
                info.path
            );
            assert!(
                app.files().exists(&info.path).await.expect("looked up"),
                "the object is at {}",
                info.path
            );
        }
    }

    async fn check_tokens(&self, name_a: &str, name_b: &str) {
        let Self {
            ts,
            b_app,
            a,
            b,
            other,
            ..
        } = self;
        let user = self.user.as_str();
        let bytes_b = b"organization B file";
        let token_b = b_app
            .store()
            .set_public_link(&b.id, PublicLinkChange::On, "media-b-read-token", user)
            .await
            .expect("B's read token")
            .token;
        let token_a = ts.share(&a.id).await;

        for client in [ts.anonymous(), ts.client2.clone()] {
            let public = client
                .get(&format!("/shared/{}?readToken={token_b}", b.id))
                .await;
            public.check_ok();
            assert_eq!(
                public.value()["createdBy"],
                "",
                "the other organization's author is hidden"
            );
            for part in ["blocks", "cards"] {
                let listed = client
                    .get(&format!("/shared/{}/{part}?readToken={token_b}", b.id))
                    .await
                    .ok_json::<Vec<serde_json::Value>>();
                assert!(
                    listed.iter().any(|item| item["id"] == self.card_b.id),
                    "B's real card is read through {part}"
                );
            }
            let path = format!("/shared/{}/files/{name_b}?readToken={token_b}", b.id);
            let file = client.get(&path).await;
            file.check_ok();
            assert_eq!(file.body, bytes_b, "the public link reads B's actual bytes");
            let range = Client::send(
                client
                    .request(Method::GET, &path)
                    .header("Range", "bytes=2-5"),
            )
            .await;
            assert_eq!(
                range.status,
                StatusCode::PARTIAL_CONTENT,
                "a token works for ranges too"
            );
            assert_eq!(range.body, &bytes_b[2..6], "the range is B's");
            let info = client
                .get(&format!(
                    "/shared/{}/files/{name_b}/info?readToken={token_b}",
                    b.id
                ))
                .await;
            info.check_ok();
            assert!(
                info.value().get("path").is_none(),
                "the object key is not serialized"
            );
            // Wrong board in B, a board in A, and a valid token with another file name.
            for forbidden in [
                format!("/shared/{}/files/{name_b}?readToken={token_b}", other.id),
                format!("/shared/{}/files/{name_a}?readToken={token_b}", a.id),
                format!("/shared/{}/files/{name_a}?readToken={token_b}", b.id),
                format!("/shared/{}/files/{name_b}/info?readToken={token_a}", b.id),
            ] {
                let denied = client.get(&forbidden).await;
                assert!(
                    !denied.status.is_success(),
                    "a token escaped its board: {forbidden}"
                );
            }
            for card in [&self.card_a, &self.card_other] {
                client
                    .get(&format!(
                        "/shared/{}/blocks?blockId={}&readToken={token_b}",
                        b.id, card.id
                    ))
                    .await
                    .check_not_found();
            }
        }
    }
    async fn cleanup(&self, name_a: &str, name_b: &str) {
        let Self {
            ts, b_app, a, b, ..
        } = self;
        let user = self.user.as_str();
        let info_a = ts
            .app
            .store()
            .get_file_record(&a.id, name_a)
            .await
            .expect("A's key");
        let info_b = b_app
            .store()
            .get_file_record(&b.id, name_b)
            .await
            .expect("B's key");
        assert_ne!(
            info_a.path, info_b.path,
            "keys differ between organizations"
        );
        assert!(
            info_a.path.contains(&a.id) && info_b.path.contains(&b.id),
            "each key includes its board UUID"
        );

        let live_a = upload(&ts.app, user, &a.id, b"live A part").await;
        let live_b = upload(b_app, user, &b.id, b"live B part").await;
        let expired = upload(b_app, user, &b.id, b"expired B part").await;
        b_app
            .store()
            .execute_unchecked(&format!(
                "UPDATE uploads SET expires_at = 1 WHERE id = '{}'",
                expired.id
            ))
            .await
            .expect("only the third upload expires");
        fvoci_server::keep_house(&ts.app).await;
        assert!(
            ts.app.upload(&live_a.id, user).await.is_ok(),
            "A's live upload survives cleanup"
        );
        assert!(
            b_app.upload(&live_b.id, user).await.is_ok(),
            "B's live upload survives cleanup"
        );
        assert!(
            b_app.upload(&expired.id, user).await.is_err(),
            "B's expired upload is removed"
        );
        assert!(
            ts.app
                .files()
                .exists(&info_a.path)
                .await
                .expect("A's file exists"),
            "cleanup retained A's referenced file"
        );
        assert!(
            b_app
                .files()
                .exists(&info_b.path)
                .await
                .expect("B's file exists"),
            "cleanup retained B's referenced file"
        );
        // Sweeping takes a System handle, so B's active upload directory is not mistaken
        // for an abandoned directory of A. No export is in progress in this test.
        ts.app
            .housekeeping()
            .sweep_temporary_files()
            .await
            .expect("the startup-style sweep");
        b_app
            .complete_upload(&live_b)
            .await
            .expect("B's parts survived the sweep");
        ts.app
            .complete_upload(&live_a)
            .await
            .expect("A's parts survived the sweep");
    }
}

async fn card(app: &App, user: &str, board: &str) -> Block {
    let mut card = Block {
        id: new_id(),
        container_id: board.to_owned(),
        block_type: BlockType::Card,
        title: "Card of its own organization".to_owned(),
        ..Block::default()
    };
    app.store()
        .insert_block(&mut card, user)
        .await
        .expect("a real card in the board");
    card
}

async fn files_and_cleanup(configure: Option<fn(&mut Configuration)>) {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let scene = Scene::new(configure).await;
    let (name_a, name_b) = scene.receive_files().await;
    scene.check_keys(&name_a, &name_b).await;
    scene.check_tokens(&name_a, &name_b).await;
    scene.cleanup(&name_a, &name_b).await;
    scene.ts.teardown().await;
}

#[tokio::test]
async fn local_files_tokens_uploads_and_housekeeping_stay_in_their_organizations() {
    Box::pin(files_and_cleanup(None)).await;
}

fn s3(config: &mut Configuration) {
    "s3".clone_into(&mut config.file_storage);
    config.upload_part_bytes = 5 * 1024 * 1024;
    config.file_storage_s3 = S3Config {
        access_key_id: "any".to_owned(),
        secret_access_key: "any".to_owned(),
        bucket: std::env::var("FVOCI_S3_TEST_BUCKET").expect("the test bucket"),
        endpoint: std::env::var("FVOCI_S3_TEST_ENDPOINT").expect("the test endpoint"),
        key_prefix: "organization-review-round2".to_owned(),
        region: "us-east-1".to_owned(),
        ..S3Config::default()
    };
}

#[tokio::test]
#[ignore = "needs the isolated S3 test service and PostgreSQL"]
async fn s3_files_tokens_uploads_and_housekeeping_stay_in_their_organizations() {
    Box::pin(files_and_cleanup(Some(s3))).await;
}

/// The upload-parent exception must not turn a mistaken tenant row into an object
/// deletion in another organization. The row is seeded through A's public Store API;
/// no current HTTP endpoint is claimed to accept this metadata from a client.
///
/// The schema refuses an upload of A that names B's board (the trigger
/// `uploads_target_of_its_organization`), so the row names A's
/// own board and only its path is B's: what the cancellation removes is the path.
#[tokio::test]
async fn cancelling_a_misbound_upload_does_not_delete_another_orgs_recorded_file() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let scene = Scene::new(None).await;
    let (_, name_b) = scene.receive_files().await;
    let victim = scene
        .b_app
        .store()
        .get_file_record(&scene.b.id, &name_b)
        .await
        .expect("B's real file");
    let mut forged = scene
        .b_app
        .create_upload(
            &scene.user,
            &scene.b.id,
            FileToUpload {
                name: "discarded.bin".to_owned(),
                size: 0,
                content_type: String::new(),
            },
        )
        .await
        .expect("well-shaped upload metadata");
    forged.id = fvoci_server::auth::secret::new_secret();
    forged.container_id = scene.a.id.clone();
    forged.user_id = scene.ts.client2.me().await.id;
    forged.path = victim.path.clone();
    forged.multipart_id = "left-over-multipart".to_owned();
    let inserted = scene.ts.app.store().create_upload(&forged).await;
    assert!(inserted.is_ok(), "A stores its upload: {inserted:?}");
    let cancelled = scene
        .ts
        .client2
        .delete(&format!("{ROOT_WS}/uploads/{}", forged.id))
        .await;
    let exists = scene
        .b_app
        .files()
        .exists(&victim.path)
        .await
        .expect("check B's object");
    let record_exists = scene
        .b_app
        .store()
        .get_file_record(&scene.b.id, &name_b)
        .await
        .is_ok();
    scene.ts.teardown().await;
    assert!(
        exists && record_exists,
        "A's upload cancellation erased B's file: seed={inserted:?}, cancel={}, object exists={exists}, B record exists={record_exists}",
        cancelled.status
    );
}

/// A board in A's trash keeps the records of its files as JSON, which no key holds: one
/// that lists the path of B's file must not make the purge of A's trash remove B's
/// object. The row is stored through A's handle, below the server's checks.
#[tokio::test]
async fn purging_a_trash_row_does_not_delete_another_orgs_file() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let scene = Scene::new(None).await;
    let (_, name_b) = scene.receive_files().await;
    let victim = scene
        .b_app
        .store()
        .get_file_record(&scene.b.id, &name_b)
        .await
        .expect("B's real file");
    let files = json!([{
        "id": new_id(), "board_id": new_id(), "stored_name": "gone.txt", "name": "gone.txt",
        "extension": ".txt", "size": 1, "path": victim.path, "created_at": 0,
    }]);
    let stored = scene
        .ts
        .app
        .store()
        .execute_unchecked(&format!(
            "INSERT INTO deleted_containers (id, workspace_id, deleted_at, deletion_id, container, members, \
             files) VALUES ('{}', '{ROOT_WORKSPACE_ID}', 0, '{}', '{{}}', '[]', '{files}')",
            new_id(),
            new_id()
        ))
        .await;
    fvoci_server::keep_house(&scene.ts.app).await;
    let exists = scene
        .b_app
        .files()
        .exists(&victim.path)
        .await
        .expect("check B's object");
    scene.ts.teardown().await;
    assert_eq!(stored.ok(), Some(1), "A stores its own trash row");
    assert!(exists, "the purge of A's trash removed B's file");
}
