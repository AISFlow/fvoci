#![cfg(feature = "db-tests")]

#[path = "revision_restore/fixture.rs"]
#[allow(dead_code)]
mod restore_fixture;

use futures_util::FutureExt;
use fvoci_server::db::backend::Backend;
use restore_fixture::support::test_collab_config;
use restore_fixture::{contract, request, server, setup, Case, Fixture};
use serde_json::json;
use std::path::PathBuf;
use uuid::Uuid;

#[tokio::test]
async fn sqlite_on_restore_commit_replay_peer_and_restart() {
    contract(false, Case::Commit).await;
}
#[tokio::test]
async fn sqlite_on_restore_committed_ambiguity() {
    contract(false, Case::Ambiguous).await;
}
#[tokio::test]
async fn sqlite_on_restore_revoked_before_apply() {
    contract(false, Case::Revoke).await;
}
#[tokio::test]
async fn sqlite_on_restore_late_failure_rolls_back() {
    contract(false, Case::Rollback).await;
}
#[tokio::test]
async fn sqlite_native_archive_separate_database_preserves_restored_history() {
    use fvoci_server::db::{
        import_jobs::claim_next_import_job_backend,
        native_archive::{
            capture_backend, publish_backend, queue_restore_backend, stage_key_backend, Selection,
        },
    };
    use fvoci_server::native_archive::{self as native, validate_native};
    use std::collections::BTreeMap;
    use tokio_util::sync::CancellationToken;
    let source = Fixture::new(false).await;
    let mut destination = Fixture::new(false).await;
    assert_ne!(source.root, destination.root);
    let src = server(&source).await;
    let dst = server(&destination).await;
    let result = std::panic::AssertUnwindSafe(async {
        let (token, workspace, _) = setup(src.addr, true).await;
        let (dest_token, _, _) = setup(dst.addr, false).await;
        let (status, dest_personal) = request(dst.addr, reqwest::Method::POST, "/api/v1/me/personal-workspace", &dest_token, None).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{dest_personal}");
        let dest_workspace = dest_personal["id"].as_str().unwrap().to_string();
        let (_, actor) = request(src.addr, reqwest::Method::GET, "/api/v1/auth/me", &token, None).await;
        let actor_id = Uuid::parse_str(actor["userId"].as_str().unwrap()).unwrap();
        let credential = Uuid::parse_str(actor["sessionId"].as_str().unwrap()).unwrap();
        let (_, dest_actor) = request(dst.addr, reqwest::Method::GET, "/api/v1/auth/me", &dest_token, None).await;
        let dest_actor_id = Uuid::parse_str(dest_actor["userId"].as_str().unwrap()).unwrap();
        let dest_credential = Uuid::parse_str(dest_actor["sessionId"].as_str().unwrap()).unwrap();
        let (status, project) = request(src.addr, reqwest::Method::POST, &format!("/api/v1/workspaces/{workspace}/projects"), &token,
            Some(json!({"key":"ARC","name":"복원할 프로젝트","visibility":"private"}))).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{project}");
        let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
        let project_base = format!("/api/v1/workspaces/{workspace}/projects/{project_id}");
        let mut documents = Vec::new();
        for title in ["아카이브 본문", "아카이브 참조"] {
            let (status, row) = request(src.addr, reqwest::Method::POST, &format!("{project_base}/documents"), &token,
                Some(json!({"title":title,"parentId":project["rootDocumentId"]}))).await;
            assert_eq!(status, reqwest::StatusCode::CREATED, "{row}");
            documents.push(row);
        }
        let doc = Uuid::parse_str(documents[0]["id"].as_str().unwrap()).unwrap();
        let reference = Uuid::parse_str(documents[1]["id"].as_str().unwrap()).unwrap();
        let attachment = Uuid::now_v7();
        let file_key = Uuid::now_v7().to_string();
        let file_bytes = "보존할 첨부 bytes\n\0🧪".as_bytes().to_vec();
        let storage: fvoci_server::attachments::ObjectStorage = fvoci_server::attachments::LocalStorage::new(source.root.join("storage")).into();
        storage.put_bytes(&file_key, file_bytes.clone()).await.unwrap();
        let Backend::Sqlite(source_pool) = &source.backend else { unreachable!() };
        sqlx::query("INSERT INTO attachments(id,workspace_id,document_id,uploader_id,status,name,mime,size_bytes,reserved_size_bytes,storage_key,completed_at) VALUES(?1,?2,?3,?4,'stored','자료.txt','text/plain',?5,?5,?6,unixepoch()*1000000)")
            .bind(attachment.as_bytes().as_slice()).bind(Uuid::parse_str(&workspace).unwrap().as_bytes().as_slice()).bind(doc.as_bytes().as_slice()).bind(actor_id.as_bytes().as_slice())
            .bind(i64::try_from(file_bytes.len()).unwrap()).bind(&file_key).execute(source_pool).await.unwrap();
        let content = json!({"type":"doc","content":[
            {"type":"paragraph","attrs":{"id":"retained-archive-block"},"content":[{"type":"text","text":"반출 전에 복원한 본문 🧪"}]},
            {"type":"paragraph","attrs":{"id":"retained-reference"},"content":[{"type":"mention","attrs":{"entity":"document","id":reference,"label":"아카이브 참조"}}]},
            {"type":"attachment","attrs":{"id":attachment,"name":"자료.txt","image":false}}
        ]});
        let body_path = format!("{project_base}/documents/{doc}");
        let (status, write) = request(src.addr, reqwest::Method::PUT, &format!("{body_path}/body"), &token, Some(json!({"contentJson":content}))).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{write}");
        let (status, revision) = request(src.addr, reqwest::Method::POST, &format!("{body_path}/revisions"), &token, None).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{revision}");
        let revision_id = revision["id"].as_str().unwrap();
        let (status, write) = request(src.addr, reqwest::Method::PUT, &format!("{body_path}/body"), &token,
            Some(json!({"contentJson":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"교체될 본문"}]}]}}))).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{write}");
        let restore_path = format!("{body_path}/revisions/{revision_id}/restore");
        let (status, preview) = request(src.addr, reqwest::Method::GET, &format!("{restore_path}-preview"), &token, None).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{preview}");
        let (status, restored) = request(src.addr, reqwest::Method::POST, &restore_path, &token,
            Some(json!({"expectedTailSeq":preview["currentTailSeq"],"correlationId":Uuid::now_v7()}))).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{restored}");
        let selection = Selection {project:project_id,zotero_connectors:&[]};
        let mut capture = capture_backend(&source.backend, Uuid::parse_str(&workspace).unwrap(), actor_id, credential, &selection).await.unwrap();
        assert_eq!(capture.archive.graph.attachments.len(), 1);
        assert_eq!(capture.file_keys.get(&attachment), Some(&file_key));
        let payload_entry = capture.archive.graph.attachments[0].payload_entry.clone();
        capture.archive.entries.insert(payload_entry.clone(), native::encode(&native::read_file(&storage, &file_key, file_bytes.len() as i64).await.unwrap()));
        let cancel = CancellationToken::new();
        let archive = validate_native(capture.archive, test_collab_config(4,60_000), &cancel).await.unwrap();
        let restore_uuid = Uuid::parse_str(restored["revisionId"].as_str().unwrap()).unwrap();
        assert!(archive.graph.revisions.iter().any(|r|r.id==restore_uuid));
        let mut missing = archive.clone(); missing.entries.remove(&payload_entry);
        assert!(missing.validate().is_err(), "missing file refuses before publication");
        let mut tampered = archive.clone(); tampered.entries.insert(payload_entry.clone(), native::encode(b"tampered"));
        assert!(tampered.validate().is_err(), "attachment length tampering refuses");
        // Existing native container reader/writer: this is the real product
        // executable, rather than the integration test runner's current_exe.
        let helper = PathBuf::from(env!("CARGO_BIN_EXE_fvoci-server"));
        let packed = native::container(&helper, serde_json::to_vec(&archive).unwrap(), true, &cancel).await.unwrap();
        let packed = native::decode(&packed).unwrap();
        let parsed = native::parse(&helper, packed.clone(), &cancel).await.unwrap();
        let parsed = validate_native(parsed, test_collab_config(4,60_000), &cancel).await.unwrap();
        let hash = native::digest(&packed);
        let dest_workspace_id = Uuid::parse_str(&dest_workspace).unwrap();
        let job = queue_restore_backend(&destination.backend, dest_workspace_id, dest_actor_id, dest_credential, Uuid::now_v7(), &hash, &packed).await.unwrap();
        let claim = claim_next_import_job_backend(&destination.backend).await.unwrap().unwrap();
        assert_eq!(claim.job_id, job);
        let dest_storage: fvoci_server::attachments::ObjectStorage = fvoci_server::attachments::LocalStorage::new(destination.root.join("storage")).into();
        let staged = Uuid::now_v7().to_string();
        stage_key_backend(&destination.backend, &claim, attachment, &staged, &cancel).await.unwrap();
        dest_storage.put_bytes(&staged, parsed.bytes(&payload_entry).unwrap()).await.unwrap();
        let keys = BTreeMap::from([(attachment,staged.clone())]);
        publish_backend(&destination.backend, &claim, &parsed, &keys, &fvoci_server::db::quota::StorageQuota::Unlimited, &hash, &cancel).await.unwrap();
        let dest_base = format!("/api/v1/workspaces/{dest_workspace}/projects/{project_id}/documents/{doc}");
        let (status, body) = request(dst.addr, reqwest::Method::GET, &format!("{dest_base}/body"), &dest_token, None).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{body}");
        assert_eq!(body["contentJson"], content);
        let (status, history) = request(dst.addr, reqwest::Method::GET, &format!("{dest_base}/revisions"), &dest_token, None).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{history}");
        assert_eq!(history["items"].as_array().unwrap().len(), archive.graph.revisions.iter().filter(|r|r.target_id==doc).count());
        for original in archive.graph.revisions.iter().filter(|r| r.target_id == doc) {
            let (status, detail) = request(dst.addr, reqwest::Method::GET, &format!("{dest_base}/revisions/{}", original.id), &dest_token, None).await;
            assert_eq!(status, reqwest::StatusCode::OK, "{detail}");
            assert_eq!(detail["id"], original.id.to_string());
            assert_eq!(detail["contentJson"], original.content_json);
            assert_eq!(detail["reason"], original.reason);
            assert_eq!(detail["restoredFromId"], json!(original.restored_from_id));
            assert_eq!(detail["createdBy"], json!(original.created_by.map(|_| dest_actor_id)));
            assert_eq!(detail["ySnapshot"], native::encode(&archive.bytes(&original.snapshot_entry).unwrap()));
            assert_eq!(chrono::DateTime::parse_from_rfc3339(detail["createdAt"].as_str().unwrap()).unwrap(), chrono::DateTime::parse_from_rfc3339(&original.created_at).unwrap());
            let metadata = destination.sql(&format!("SELECT json_object('text',text,'encoding',encoding,'correlation',hex(restore_correlation_id),'base',restore_base_tail_seq,'committed',restore_committed_tail_seq) FROM revisions WHERE id=X'{}'", original.id.simple())).await;
            assert_eq!(metadata["text"], original.text);
            assert_eq!(metadata["encoding"], original.encoding);
            assert_eq!(metadata["correlation"], original.restore_correlation_id.map(|id| id.simple().to_string().to_uppercase()).unwrap_or_default());
            assert_eq!(metadata["base"], json!(original.restore_base_tail_seq));
            assert_eq!(metadata["committed"], json!(original.restore_committed_tail_seq));
        }
        let (status, detail) = request(dst.addr, reqwest::Method::GET, &format!("{dest_base}/revisions/{restore_uuid}"), &dest_token, None).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{detail}");
        assert_eq!(detail["restoredFromId"], revision["id"]);
        assert_eq!(detail["contentJson"], content);
        assert_eq!(native::read_file(&dest_storage, &staged, file_bytes.len() as i64).await.unwrap(), file_bytes);
        let (status, _) = request(dst.addr, reqwest::Method::GET, &format!("{dest_base}/body"), "", None).await;
        assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
        let outsider = Uuid::now_v7();
        let outsider_token = fvoci_server::auth::token::new_token();
        let Backend::Sqlite(dest_pool) = &destination.backend else { unreachable!() };
        sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,'outsider@example.invalid','Outsider')")
            .bind(outsider.as_bytes().as_slice()).execute(dest_pool).await.unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'member')")
            .bind(dest_workspace_id.as_bytes().as_slice()).bind(outsider.as_bytes().as_slice()).execute(dest_pool).await.unwrap();
        sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,unixepoch()*1000000+3600000000)")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(outsider.as_bytes().as_slice()).bind(&outsider_token.hash).execute(dest_pool).await.unwrap();
        let (status, _) = request(dst.addr, reqwest::Method::GET, &format!("{dest_base}/body"), &outsider_token.token, None).await;
        assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "ordinary workspace member cannot read the restored private project");
        (dest_token, dest_base, content, attachment, staged, file_bytes)
    }).catch_unwind().await;
    src.shutdown().await.unwrap();
    dst.shutdown().await.unwrap();
    let restarted_result = if let Ok((token, base, expected, attachment, key, bytes)) = &result {
        destination.reopen().await;
        let restarted = server(&destination).await;
        let readback = std::panic::AssertUnwindSafe(async {
            let (status, body) = request(
                restarted.addr,
                reqwest::Method::GET,
                &format!("{base}/body"),
                token,
                None,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::OK, "{body}");
            assert_eq!(body["contentJson"], *expected);
            let storage: fvoci_server::attachments::ObjectStorage =
                fvoci_server::attachments::LocalStorage::new(destination.root.join("storage"))
                    .into();
            assert_eq!(
                native::read_file(&storage, key, bytes.len() as i64)
                    .await
                    .unwrap(),
                *bytes
            );
            let (status, detail) = request(
                restarted.addr,
                reqwest::Method::GET,
                &format!(
                    "/api/v1/workspaces/{}/attachments/{attachment}",
                    base.split('/').nth(4).unwrap()
                ),
                token,
                None,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::OK, "{detail}");
            assert_eq!(detail["id"], attachment.to_string());
            assert_eq!(detail["name"], "자료.txt");
            assert_eq!(detail["mime"], "text/plain");
            assert_eq!(detail["sizeBytes"], bytes.len());
        })
        .catch_unwind()
        .await;
        restarted.shutdown().await.unwrap();
        readback
    } else {
        Ok(())
    };
    source.finish().await;
    destination.finish().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    if let Err(panic) = restarted_result {
        std::panic::resume_unwind(panic);
    }
}
