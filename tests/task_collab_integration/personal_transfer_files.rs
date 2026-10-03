//! W2 attachment MOVE (lease msg_ac45a56bab8c): the same attachment UUID
//! moves with fresh storage keys staged before the transaction and published
//! inside it; the old keys are journaled in the source workspace. COPY keeps
//! refusing files, and a moved body may only show files that move with it.

use super::*;
use sha2::{Digest, Sha256};

/// A real upload through the product routes: create, PUT each part, complete.
async fn upload_task_file(
    addr: SocketAddr,
    token: &str,
    workspace: Uuid,
    task: &str,
    name: &str,
    bytes: &[u8],
) -> Uuid {
    let (status, created) = session_call(
        addr,
        Method::POST,
        &format!("/api/v1/workspaces/{workspace}/tasks/{task}/uploads"),
        token,
        Some(json!({"name": name, "sizeBytes": bytes.len()})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let attachment = created["attachmentId"].as_str().unwrap().to_string();
    let part_size = created["partSizeBytes"].as_u64().unwrap() as usize;
    let client = reqwest::Client::new();
    let mut parts = Vec::new();
    for part in created["parts"].as_array().unwrap() {
        let number = part["partNumber"].as_u64().unwrap() as usize;
        let start = (number - 1) * part_size;
        let end = (start + part_size).min(bytes.len());
        let url = part["url"].as_str().unwrap();
        let url = if url.starts_with("http") {
            url.to_string()
        } else {
            format!("http://{addr}{url}")
        };
        let response = client
            .put(url)
            .header("origin", PUBLIC_ORIGIN)
            .header("cookie", format!("fvoci_session={token}"))
            .header("content-type", "application/octet-stream")
            .body(bytes[start..end].to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "part {number}");
        let etag = response
            .headers()
            .get("etag")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        parts.push(json!({"partNumber": number, "etag": etag}));
    }
    let (status, completed) = session_call(
        addr,
        Method::POST,
        &format!("/api/v1/workspaces/{workspace}/attachments/{attachment}/complete"),
        token,
        Some(json!({"parts": parts})),
    )
    .await;
    assert!(status.is_success(), "{status}: {completed}");
    Uuid::parse_str(&attachment).unwrap()
}

/// The stored object as an authorized client downloads it.
async fn download(
    addr: SocketAddr,
    token: &str,
    workspace: Uuid,
    attachment: Uuid,
) -> (StatusCode, Vec<u8>) {
    let response = reqwest::Client::new()
        .get(format!(
            "http://{addr}/api/v1/workspaces/{workspace}/attachments/{attachment}/download"
        ))
        .header("origin", PUBLIC_ORIGIN)
        .header("cookie", format!("fvoci_session={token}"))
        .send()
        .await
        .unwrap();
    let status = response.status();
    (status, response.bytes().await.unwrap().to_vec())
}

/// Committed attachment row and cleanup journal of a workspace (observer).
async fn file_rows(
    run: &TestRun,
    workspace: Uuid,
    attachment: Uuid,
) -> (Option<String>, Vec<String>) {
    let admin = admin_pool(&run.harness).await;
    let key: Option<String> = sqlx::query_scalar(
        "SELECT storage_key FROM fvoci.attachments WHERE workspace_id=$1 AND id=$2",
    )
    .bind(workspace)
    .bind(attachment)
    .fetch_optional(&admin)
    .await
    .unwrap();
    let journal: Vec<String> = sqlx::query_scalar(
        "SELECT storage_key FROM fvoci.attachment_object_cleanups WHERE workspace_id=$1 AND attachment_id=$2 ORDER BY storage_key",
    )
    .bind(workspace)
    .bind(attachment)
    .fetch_all(&admin)
    .await
    .unwrap();
    admin.close().await;
    (key, journal)
}

#[tokio::test]
async fn personal_transfer_move_carries_attachments_with_fresh_keys() {
    run_test("personal_transfer_move_carries_attachments_with_fresh_keys", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, _, _, mut selection) = setup_transfer(&mut run).await;
        let task = selection["taskId"].as_str().unwrap().to_string();
        let bytes: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let attachment = upload_task_file(addr, &actor.session_token, source, &task, "증빙 자료 📎.bin", &bytes).await;
        let (old_key, source_journal) = file_rows(&run, source, attachment).await;
        let old_key = old_key.expect("source row");
        assert!(source_journal.is_empty());
        let (status, before) = download(addr, &actor.session_token, source, attachment).await;
        assert_eq!((status, before.len()), (StatusCode::OK, bytes.len()));
        // The body shows the file through the product body write (native
        // history), a manual revision keeps it, and a later edit still shows it.
        let document = Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
        let node = json!({"type":"attachment","attrs":{"id":attachment.to_string(),"name":"증빙 자료 📎.bin"}});
        let first = json!({"type":"doc","content":[para(&Uuid::now_v7().to_string(), "첨부 전 문단"), node]});
        let (status, saved) = session_call(addr, Method::PUT, &document_api(source, document, "/body"), &actor.session_token, Some(json!({"contentJson":first}))).await;
        assert!(status.is_success(), "{status}: {saved}");
        let (status, revision) = session_call(addr, Method::POST, &document_api(source, document, "/revisions"), &actor.session_token, None).await;
        assert_eq!(status, StatusCode::CREATED, "{revision}");
        let second = json!({"type":"doc","content":[para(&Uuid::now_v7().to_string(), "첨부 뒤 수정"), node]});
        let (status, saved) = session_call(addr, Method::PUT, &document_api(source, document, "/body"), &actor.session_token, Some(json!({"contentJson":second}))).await;
        assert!(status.is_success(), "{status}: {saved}");
        let shown = |graph: &Value, table: &str| -> usize {
            graph[table].as_array().unwrap().iter().filter(|row| row.to_string().contains(&attachment.to_string())).count()
        };
        let source_graph = transfer_graph(&run, source).await;
        let revisions_before = rows_of(&source_graph, "revisions", "target_id", document, None);
        assert!(shown(&source_graph, "revisions") >= 1, "a revision references the file");
        assert!(shown(&source_graph, "documents") >= 1, "the current body shows the file");
        assert!(!source_graph["document_collab_updates"].as_array().unwrap().is_empty() || !source_graph["document_states"].as_array().unwrap().is_empty(), "native history exists");
        selection_versions(&run, source, &mut selection).await;
        let (status, preview) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(selection.clone())).await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        assert_eq!(preview["attachmentCount"], 1);
        assert!(preview["dispositions"].as_array().unwrap().iter().any(|d| d["item"] == "attachment" && d["outcome"] == "moved" && d["count"] == 1), "{preview}");
        let body = json!({"requestId":Uuid::now_v7(),"selection":selection,"previewDigest":preview["digest"],"confirmed":true});
        let (status, moved) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{moved}");
        let team = actor.workspace_id;
        // Same UUID in the destination with a fresh key and no staging journal.
        let (new_key, team_journal) = file_rows(&run, team, attachment).await;
        let new_key = new_key.expect("destination row");
        assert_ne!(new_key, old_key);
        assert!(team_journal.is_empty(), "staged journal rows are consumed: {team_journal:?}");
        // The old key is journaled in the source, where no row references it.
        let (gone, source_journal) = file_rows(&run, source, attachment).await;
        assert_eq!(gone, None);
        assert_eq!(source_journal, vec![old_key]);
        // The moved body and retained history keep the same file and revisions.
        let team_graph = transfer_graph(&run, team).await;
        assert_eq!(rows_of(&team_graph, "revisions", "target_id", document, Some(source)), revisions_before);
        assert!(shown(&team_graph, "revisions") >= 1);
        assert!(shown(&team_graph, "documents") >= 1);
        // A fresh client reads the exact bytes from the team; the source is gone.
        let (status, after) = download(addr, &actor.session_token, team, attachment).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(Sha256::digest(&after), Sha256::digest(&bytes), "exact bytes after the move");
        let (status, _) = download(addr, &actor.session_token, source, attachment).await;
        assert!(matches!(status, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN), "{status}");
        run.finish().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn personal_transfer_copy_with_attachment_and_foreign_body_file_refuse_before_effects() {
    run_test("personal_transfer_copy_with_attachment_and_foreign_body_file_refuse_before_effects", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, _, _, mut selection) = setup_transfer(&mut run).await;
        let team = actor.workspace_id;
        let document = Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
        let task = selection["taskId"].as_str().unwrap().to_string();
        // A body (written through the product body write, so retained native
        // history) showing a file that does not move with the pair.
        let foreign = Uuid::now_v7().to_string();
        let shown = json!({"type":"doc","content":[{"type":"attachment","attrs":{"id":foreign,"name":"다른 파일.pdf"}}]});
        let (status, saved) = session_call(addr, Method::PUT, &document_api(source, document, "/body"), &actor.session_token, Some(json!({"contentJson":shown.clone()}))).await;
        assert!(status.is_success(), "{status}: {saved}");
        selection_versions(&run, source, &mut selection).await;
        let (source_before, team_before) = (transfer_graph(&run, source).await, transfer_graph(&run, team).await);
        let (status, error) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(selection.clone())).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["params"]["code"], "file");
        assert_eq!(error["title"], "retained file outside the moved pair");
        assert!(!error.to_string().contains(&foreign), "no foreign file identifier is disclosed");
        assert_eq!(transfer_graph(&run, source).await, source_before);
        assert_eq!(transfer_graph(&run, team).await, team_before);
        // Retained only: a revision keeps the foreign file while the current
        // body no longer shows it; the retained history still refuses as a file.
        let (status, revision) = session_call(addr, Method::POST, &document_api(source, document, "/revisions"), &actor.session_token, None).await;
        assert_eq!(status, StatusCode::CREATED, "{revision}");
        let clean = json!({"type":"doc","content":[para(&Uuid::now_v7().to_string(), "파일 없는 본문")]});
        let (status, saved) = session_call(addr, Method::PUT, &document_api(source, document, "/body"), &actor.session_token, Some(json!({"contentJson":clean}))).await;
        assert!(status.is_success(), "{status}: {saved}");
        selection_versions(&run, source, &mut selection).await;
        let (source_before, team_before) = (transfer_graph(&run, source).await, transfer_graph(&run, team).await);
        let (status, error) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(selection.clone())).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["params"]["code"], "file");
        assert_eq!(error["title"], "retained file outside the moved pair");
        assert!(!error.to_string().contains(&foreign));
        assert_eq!(transfer_graph(&run, source).await, source_before);
        assert_eq!(transfer_graph(&run, team).await, team_before);
        // COPY of a pair with its own file, whose CURRENT body shows a file
        // outside the pair: the copy cannot map that node, so it refuses as a
        // file before effects (the pair's own file would be copied).
        upload_task_file(addr, &actor.session_token, source, &task, "copy.bin", b"copy").await;
        let (status, saved) = session_call(addr, Method::PUT, &document_api(source, document, "/body"), &actor.session_token, Some(json!({"contentJson":shown}))).await;
        assert!(status.is_success(), "{status}: {saved}");
        selection_versions(&run, source, &mut selection).await;
        let (source_before, team_before) = (transfer_graph(&run, source).await, transfer_graph(&run, team).await);
        let mut copy = selection.clone();
        copy["action"] = json!("copy");
        let (status, error) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(copy)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["params"]["code"], "file");
        assert_eq!(error["title"], "copy body file outside the pair");
        assert!(!error.to_string().contains(&foreign));
        assert_eq!(transfer_graph(&run, source).await, source_before);
        assert_eq!(transfer_graph(&run, team).await, team_before);
        run.finish().await.unwrap();
    })
    .await;
}

/// The same server as `setup_task`, with storage at a root this test can run
/// the reclaim job against and a chosen quota. Returns the team task too.
async fn files_server(
    run: &mut TestRun,
    quota: fvoci_server::db::quota::StorageQuota,
) -> (
    SocketAddr,
    SessionFixture,
    Uuid,
    fvoci_server::attachments::ObjectStorage,
) {
    let owner = setup_owner_session(&run.harness).await;
    let mut cfg = test_collab_config(8, 60_000);
    cfg.revoke_poll_ms = 5000;
    let (mut state, hub) = collab_app_state(&run.harness.app_url, cfg).await;
    let root = std::env::temp_dir().join(format!("fvoci-w2-files-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    state.storage = fvoci_server::attachments::ObjectStorage::local(root.clone());
    state.quota = quota;
    let addr = run.spawn_router_state(state, hub).await;
    let project = create_project(addr, &owner, "TCOL").await;
    let task = create_task(addr, &owner, project).await;
    (
        addr,
        owner,
        task,
        fvoci_server::attachments::ObjectStorage::local(root),
    )
}

/// Holds the destination workspace storage lock (the one uploads and the
/// transfer's quota admission take) in an open transaction.
async fn hold_storage(
    pool: &PgPool,
    workspace: Uuid,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(fvoci_server::attachments::STORAGE_LOCK_NAMESPACE)
        .bind(fvoci_server::db::context::lock_key_from_uuid(workspace))
        .execute(&mut *holder)
        .await
        .unwrap();
    holder
}

/// Waits (bounded) until another session waits for that storage lock: the
/// transfer has been admitted, has staged (its attachment lock released)
/// and is in its publishing transaction.
async fn wait_for_storage_waiter(run: &TestRun, workspace: Uuid) {
    let admin = admin_pool(&run.harness).await;
    for _ in 0..400 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_locks WHERE locktype='advisory' AND NOT granted AND classid=$1::int4::oid AND objid=$2::int4::oid AND objsubid=2",
        )
        .bind(fvoci_server::attachments::STORAGE_LOCK_NAMESPACE)
        .bind(fvoci_server::db::context::lock_key_from_uuid(workspace))
        .fetch_one(&admin)
        .await
        .unwrap();
        if waiting > 0 {
            admin.close().await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the transfer never reached publication");
}

/// Waits (bounded) until the transfer has journaled its staged key(s).
async fn staged_keys(run: &TestRun, team: Uuid, attachment: Uuid) -> Vec<String> {
    for _ in 0..400 {
        let (_, journal) = file_rows(run, team, attachment).await;
        if !journal.is_empty() {
            return journal;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the transfer never staged {attachment}");
}

/// Test control standing in for the staging grace having passed, then the
/// existing reclaim job (restricted role) over this attachment's journal.
async fn reclaim_now(
    run: &TestRun,
    actor: &SessionFixture,
    storage: &fvoci_server::attachments::ObjectStorage,
    team: Uuid,
    attachment: Uuid,
) -> u32 {
    let admin = admin_pool(&run.harness).await;
    sqlx::query("UPDATE fvoci.attachment_object_cleanups SET due_at = clock_timestamp() WHERE workspace_id=$1 AND attachment_id=$2")
        .bind(team)
        .bind(attachment)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    fvoci_server::db::attachments::reclaim_attachment_objects(
        &actor.pool,
        storage,
        Some((team, attachment)),
        10,
    )
    .await
    .unwrap()
    .reclaimed
}

#[tokio::test]
async fn personal_transfer_move_refuses_a_staged_file_reclaimed_before_commit() {
    run_test("personal_transfer_move_refuses_a_staged_file_reclaimed_before_commit", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, _, storage) = files_server(&mut run, Default::default()).await;
        let (source, _, _, mut selection) = transfer_pair(addr, &actor).await;
        let team = actor.workspace_id;
        let task = selection["taskId"].as_str().unwrap().to_string();
        let bytes = b"reclaimed before commit".repeat(1000);
        let attachment = upload_task_file(addr, &actor.session_token, source, &task, "회수.bin", &bytes).await;
        let (old_key, _) = file_rows(&run, source, attachment).await;
        selection_versions(&run, source, &mut selection).await;
        let (status, preview) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(selection.clone())).await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        let (source_before, team_before) = (transfer_graph(&run, source).await, transfer_graph(&run, team).await);
        // Hold the destination storage lock, which only the publishing
        // transaction takes, after admission and staging.
        let holder = hold_storage(&actor.pool, team).await;
        let body = json!({"requestId":Uuid::now_v7(),"selection":selection,"previewDigest":preview["digest"],"confirmed":true});
        let token = actor.session_token.clone();
        let commit = tokio::spawn(async move { session_call(addr, Method::POST, &path(source), &token, Some(body)).await });
        wait_for_storage_waiter(&run, team).await;
        let staged = staged_keys(&run, team, attachment).await;
        assert_eq!(staged.len(), 1);
        assert_eq!(storage.head(&staged[0]).await.unwrap(), Some(bytes.len() as u64));
        assert_eq!(reclaim_now(&run, &actor, &storage, team, attachment).await, 1);
        assert_eq!(storage.head(&staged[0]).await.unwrap(), None, "the unreferenced staged key is gone");
        holder.commit().await.unwrap();
        let (status, error) = commit.await.unwrap();
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["params"]["code"], "file");
        assert_eq!(error["title"], "staged file reclaimed");
        assert_eq!(transfer_graph(&run, source).await, source_before);
        assert_eq!(transfer_graph(&run, team).await, team_before);
        assert_eq!(file_rows(&run, source, attachment).await.0, old_key);
        let (status, still) = download(addr, &actor.session_token, source, attachment).await;
        assert_eq!((status, still), (StatusCode::OK, bytes));
        run.finish().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn personal_transfer_move_refuses_over_destination_quota_and_staged_keys_reclaim() {
    run_test("personal_transfer_move_refuses_over_destination_quota_and_staged_keys_reclaim", async {
        use fvoci_server::db::quota::{QuotaLimit, StorageQuota};
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (team_existing, moved) = (vec![7u8; 4000], vec![9u8; 5000]);
        // Each workspace admits its own uploads; the team cannot take both.
        let quota = StorageQuota::fixed(QuotaLimit::Bytes(4000 + 5000 - 1), QuotaLimit::Unlimited);
        let (addr, actor, team_task, storage) = files_server(&mut run, quota).await;
        let (source, _, _, mut selection) = transfer_pair(addr, &actor).await;
        let team = actor.workspace_id;
        upload_task_file(addr, &actor.session_token, team, &team_task.to_string(), "팀 파일.bin", &team_existing).await;
        let task = selection["taskId"].as_str().unwrap().to_string();
        let attachment = upload_task_file(addr, &actor.session_token, source, &task, "개인 파일.bin", &moved).await;
        selection_versions(&run, source, &mut selection).await;
        let (status, preview) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(selection.clone())).await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        let (source_before, team_before) = (transfer_graph(&run, source).await, transfer_graph(&run, team).await);
        let body = json!({"requestId":Uuid::now_v7(),"selection":selection,"previewDigest":preview["digest"],"confirmed":true});
        let (status, error) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["params"]["code"], "file");
        assert_eq!(error["title"], "destination storage limit");
        assert_eq!(transfer_graph(&run, source).await, source_before);
        assert_eq!(transfer_graph(&run, team).await, team_before);
        // The refused transfer left only its journaled, unreferenced copy.
        let (_, staged) = file_rows(&run, team, attachment).await;
        assert_eq!(staged.len(), 1);
        assert_eq!(storage.head(&staged[0]).await.unwrap(), Some(moved.len() as u64));
        assert_eq!(reclaim_now(&run, &actor, &storage, team, attachment).await, 1);
        assert_eq!(storage.head(&staged[0]).await.unwrap(), None);
        assert!(file_rows(&run, team, attachment).await.1.is_empty());
        let (status, still) = download(addr, &actor.session_token, source, attachment).await;
        assert_eq!((status, still), (StatusCode::OK, moved.clone()));
        // A COPY over the same quota refuses the same way; its staged copy is
        // journaled under the copy's NEW attachment id and reclaimed alike.
        let mut copy = selection.clone();
        copy["action"] = json!("copy");
        let (status, preview) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(copy.clone())).await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        let (source_before, team_before) = (transfer_graph(&run, source).await, transfer_graph(&run, team).await);
        let body = json!({"requestId":Uuid::now_v7(),"selection":copy,"previewDigest":preview["digest"],"confirmed":true});
        let (status, error) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["title"], "destination storage limit");
        assert_eq!(transfer_graph(&run, source).await, source_before);
        assert_eq!(transfer_graph(&run, team).await, team_before);
        let admin = admin_pool(&run.harness).await;
        let journaled: Vec<(Uuid, String)> = sqlx::query_as("SELECT attachment_id, storage_key FROM fvoci.attachment_object_cleanups WHERE workspace_id=$1")
            .bind(team)
            .fetch_all(&admin)
            .await
            .unwrap();
        admin.close().await;
        assert_eq!(journaled.len(), 1, "{journaled:?}");
        let (copy_id, copy_key) = journaled[0].clone();
        assert_ne!(copy_id, attachment, "the copy's staged key is journaled under its new id");
        assert_eq!(storage.head(&copy_key).await.unwrap(), Some(moved.len() as u64));
        assert_eq!(reclaim_now(&run, &actor, &storage, team, copy_id).await, 1);
        assert_eq!(storage.head(&copy_key).await.unwrap(), None);
        let (status, still) = download(addr, &actor.session_token, source, attachment).await;
        assert_eq!((status, still), (StatusCode::OK, moved));
        run.finish().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn personal_transfer_cancelled_move_leaves_only_reclaimable_staged_keys() {
    run_test(
        "personal_transfer_cancelled_move_leaves_only_reclaimable_staged_keys",
        async {
            let mut run = TestRun::new(TestDb::bootstrap().await);
            let (addr, actor, _, storage) = files_server(&mut run, Default::default()).await;
            let (source, _, _, mut selection) = transfer_pair(addr, &actor).await;
            let team = actor.workspace_id;
            let task = selection["taskId"].as_str().unwrap().to_string();
            let bytes = b"cancelled".repeat(2000);
            let attachment = upload_task_file(
                addr,
                &actor.session_token,
                source,
                &task,
                "취소.bin",
                &bytes,
            )
            .await;
            selection_versions(&run, source, &mut selection).await;
            let body = move_body(addr, &actor, source, &selection).await;
            let parsed: fvoci_server::api::personal_transfer::PersonalTransferBody =
                serde_json::from_value(body).unwrap();
            let (source_before, team_before) = (
                transfer_graph(&run, source).await,
                transfer_graph(&run, team).await,
            );
            let hub = run.hub();
            let engine = fvoci_server::db::personal_transfer::TransferBodyEngine {
                engine_bin: hub.engine_bin(),
                limits: hub.limits(),
            };
            let pool = PgPoolOptions::new()
                .max_connections(2)
                .connect(&run.harness.app_url)
                .await
                .unwrap();
            let holder = hold_storage(&actor.pool, team).await;
            let (user, session, task_storage) = (actor.user_id, actor.session_id, storage.clone());
            let caller = tokio::spawn(async move {
                let quota = fvoci_server::db::quota::StorageQuota::default();
                let files = fvoci_server::db::personal_transfer::TransferFiles {
                    storage: &task_storage,
                    quota: &quota,
                };
                fvoci_server::db::personal_transfer::transfer_personal_item(
                    &pool,
                    source,
                    user,
                    session,
                    &parsed,
                    None,
                    "web",
                    Some(&engine),
                    Some(files),
                )
                .await
            });
            wait_for_storage_waiter(&run, team).await;
            let staged = staged_keys(&run, team, attachment).await;
            // The caller is cancelled while its transaction waits after staging.
            caller.abort();
            assert!(caller.await.unwrap_err().is_cancelled());
            holder.commit().await.unwrap();
            assert_eq!(transfer_graph(&run, source).await, source_before);
            assert_eq!(transfer_graph(&run, team).await, team_before);
            assert_eq!(
                storage.head(&staged[0]).await.unwrap(),
                Some(bytes.len() as u64)
            );
            assert_eq!(
                reclaim_now(&run, &actor, &storage, team, attachment).await,
                u32::try_from(staged.len()).unwrap()
            );
            assert_eq!(storage.head(&staged[0]).await.unwrap(), None);
            let (status, still) = download(addr, &actor.session_token, source, attachment).await;
            assert_eq!((status, still), (StatusCode::OK, bytes));
            run.finish().await.unwrap();
        },
    )
    .await;
}

#[tokio::test]
async fn personal_transfer_move_file_changed_after_preview_is_stale_before_staging() {
    run_test("personal_transfer_move_file_changed_after_preview_is_stale_before_staging", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, _, _, mut selection) = setup_transfer(&mut run).await;
        let team = actor.workspace_id;
        let task = selection["taskId"].as_str().unwrap().to_string();
        let attachment = upload_task_file(addr, &actor.session_token, source, &task, "삭제될 파일.bin", b"stale").await;
        selection_versions(&run, source, &mut selection).await;
        let (status, preview) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(selection.clone())).await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        // A supported change after the review: the file is deleted.
        let (status, deleted) = session_call(addr, Method::DELETE, &format!("/api/v1/workspaces/{source}/attachments/{attachment}"), &actor.session_token, None).await;
        assert!(status.is_success(), "{status}: {deleted}");
        let (source_before, team_before) = (transfer_graph(&run, source).await, transfer_graph(&run, team).await);
        let body = json!({"requestId":Uuid::now_v7(),"selection":selection,"previewDigest":preview["digest"],"confirmed":true});
        let (status, error) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["params"]["code"], "preview_stale");
        assert_eq!(transfer_graph(&run, source).await, source_before);
        assert_eq!(transfer_graph(&run, team).await, team_before);
        // Admission refused before staging: nothing was journaled in the team.
        assert!(file_rows(&run, team, attachment).await.1.is_empty());
        run.finish().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn personal_transfer_staging_uses_one_connection_and_fences_reclaim_while_writing() {
    run_test("personal_transfer_staging_uses_one_connection_and_fences_reclaim_while_writing", async {
        use fvoci_server::db::attachments::{stage_attachment_for_transfer, StageAttachmentError};
        use sqlx::{Connection, PgConnection};
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, _, storage) = files_server(&mut run, Default::default()).await;
        let (source, _, _, selection) = transfer_pair(addr, &actor).await;
        let team = actor.workspace_id;
        let task = selection["taskId"].as_str().unwrap().to_string();
        let bytes = b"one connection".repeat(500);
        let attachment = upload_task_file(addr, &actor.session_token, source, &task, "한 연결.bin", &bytes).await;
        // A pool of exactly one connection: the lock's connection must carry
        // every check and the journaling (no second acquisition).
        let one = PgPoolOptions::new().max_connections(1).connect(&run.harness.app_url).await.unwrap();
        let staged = stage_attachment_for_transfer(&one, &storage, source, team, actor.user_id, actor.session_id, attachment, attachment)
            .await
            .unwrap()
            .expect("staged on one connection");
        assert_eq!(staged.original.size_bytes, bytes.len() as i64);
        assert_eq!(staged.original.sha256, <[u8; 32]>::from(Sha256::digest(&bytes)));
        assert_eq!(storage.head(&staged.original.key).await.unwrap(), Some(bytes.len() as u64));
        assert_eq!(file_rows(&run, team, attachment).await.1, vec![staged.original.key.clone()]);
        // Typed denial on the same single connection: a session that is not live.
        assert_eq!(
            stage_attachment_for_transfer(&one, &storage, source, team, actor.user_id, Uuid::now_v7(), attachment, attachment).await.unwrap().err(),
            Some(StageAttachmentError::Forbidden)
        );
        // A live writer holds the attachment session lock (as staging does
        // while it streams): staging refuses, and the reclaim job finds the
        // due staged key busy and leaves it.
        let mut writer = PgConnection::connect(&run.harness.app_url).await.unwrap();
        let held: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1, $2)")
            .bind(fvoci_server::attachments::ATTACHMENT_LOCK_NAMESPACE)
            .bind(fvoci_server::db::context::lock_key_from_uuid(attachment))
            .fetch_one(&mut writer)
            .await
            .unwrap();
        assert!(held);
        assert_eq!(
            stage_attachment_for_transfer(&one, &storage, source, team, actor.user_id, actor.session_id, attachment, attachment).await.unwrap().err(),
            Some(StageAttachmentError::NotReady)
        );
        let admin = admin_pool(&run.harness).await;
        sqlx::query("UPDATE fvoci.attachment_object_cleanups SET due_at = clock_timestamp() WHERE workspace_id=$1 AND attachment_id=$2")
            .bind(team)
            .bind(attachment)
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        let stats = fvoci_server::db::attachments::reclaim_attachment_objects(&actor.pool, &storage, Some((team, attachment)), 10)
            .await
            .unwrap();
        assert_eq!((stats.reclaimed, stats.busy), (0, 1));
        assert_eq!(storage.head(&staged.original.key).await.unwrap(), Some(bytes.len() as u64));
        writer.close().await.unwrap();
        assert_eq!(reclaim_now(&run, &actor, &storage, team, attachment).await, 1);
        assert_eq!(storage.head(&staged.original.key).await.unwrap(), None);
        // A COPY journals its fresh key under the copy's NEW id, which is the
        // id the reclaim job locks: staging must hold that id too. Held by
        // another session -> NotReady and nothing journaled under it.
        let copy_id = Uuid::now_v7();
        let lock_of = |id: Uuid| {
            sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1, $2)")
                .bind(fvoci_server::attachments::ATTACHMENT_LOCK_NAMESPACE)
                .bind(fvoci_server::db::context::lock_key_from_uuid(id))
        };
        let mut other = PgConnection::connect(&run.harness.app_url).await.unwrap();
        assert!(lock_of(copy_id).fetch_one(&mut other).await.unwrap());
        assert_eq!(
            stage_attachment_for_transfer(&one, &storage, source, team, actor.user_id, actor.session_id, attachment, copy_id).await.unwrap().err(),
            Some(StageAttachmentError::NotReady)
        );
        assert!(file_rows(&run, team, copy_id).await.1.is_empty());
        other.close().await.unwrap();
        // In flight: staging under the copy id pauses after journaling and
        // before any byte is written (db-tests barrier). While it owns both
        // the source and the copy id, neither lock is free elsewhere and the
        // due journaled key is busy for the reclaim job.
        let mut barrier = fvoci_server::db::attachments::test_barrier::arm_transfer_stage_copy(copy_id);
        let (pool_one, storage_one, user, session) = (one.clone(), storage.clone(), actor.user_id, actor.session_id);
        let staging = tokio::spawn(async move {
            stage_attachment_for_transfer(&pool_one, &storage_one, source, team, user, session, attachment, copy_id).await
        });
        barrier.wait_entered().await.unwrap();
        let journaled = file_rows(&run, team, copy_id).await.1;
        assert_eq!(journaled.len(), 1, "journaled under the copy id before the write");
        assert_eq!(storage.head(&journaled[0]).await.unwrap(), None, "nothing written yet");
        let mut probe = PgConnection::connect(&run.harness.app_url).await.unwrap();
        assert!(!lock_of(attachment).fetch_one(&mut probe).await.unwrap(), "source id held by staging");
        assert!(!lock_of(copy_id).fetch_one(&mut probe).await.unwrap(), "copy id held by staging");
        let admin = admin_pool(&run.harness).await;
        sqlx::query("UPDATE fvoci.attachment_object_cleanups SET due_at = clock_timestamp() WHERE workspace_id=$1 AND attachment_id=$2")
            .bind(team)
            .bind(copy_id)
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        let stats = fvoci_server::db::attachments::reclaim_attachment_objects(&actor.pool, &storage, Some((team, copy_id)), 10)
            .await
            .unwrap();
        assert_eq!((stats.reclaimed, stats.busy), (0, 1), "the in-flight staged key is fenced");
        barrier.proceed();
        let copied = staging.await.unwrap().unwrap().expect("staged under the copy id");
        assert_eq!(copied.original.key, journaled[0]);
        assert_eq!(storage.head(&copied.original.key).await.unwrap(), Some(bytes.len() as u64));
        // Both ids are released (collision or re-entry included: the lock
        // connection closes on release).
        assert!(lock_of(attachment).fetch_one(&mut probe).await.unwrap());
        assert!(lock_of(copy_id).fetch_one(&mut probe).await.unwrap());
        probe.close().await.unwrap();
        assert_eq!(reclaim_now(&run, &actor, &storage, team, copy_id).await, 1);
        assert_eq!(storage.head(&copied.original.key).await.unwrap(), None);
        // A copy id whose lock key collides with the source id (lock keys are
        // the UUID's low 32 bits): the same session re-enters its own lock,
        // so staging succeeds, and both acquisitions are released.
        let mut colliding = *Uuid::now_v7().as_bytes();
        colliding[12..].copy_from_slice(&attachment.as_bytes()[12..]);
        let colliding = Uuid::from_bytes(colliding);
        assert_ne!(colliding, attachment);
        assert_eq!(
            fvoci_server::db::context::lock_key_from_uuid(colliding),
            fvoci_server::db::context::lock_key_from_uuid(attachment)
        );
        let collided = stage_attachment_for_transfer(&one, &storage, source, team, actor.user_id, actor.session_id, attachment, colliding)
            .await
            .unwrap()
            .expect("re-entrant on the colliding key");
        let mut probe = PgConnection::connect(&run.harness.app_url).await.unwrap();
        assert!(lock_of(attachment).fetch_one(&mut probe).await.unwrap(), "colliding key released");
        probe.close().await.unwrap();
        assert_eq!(reclaim_now(&run, &actor, &storage, team, colliding).await, 1);
        assert_eq!(storage.head(&collided.original.key).await.unwrap(), None);
        one.close().await;
        run.finish().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn personal_transfer_copy_carries_attachments_with_new_ids() {
    run_test("personal_transfer_copy_carries_attachments_with_new_ids", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, _, _, mut selection) = setup_transfer(&mut run).await;
        let team = actor.workspace_id;
        let document = Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
        let task = selection["taskId"].as_str().unwrap().to_string();
        let bytes: Vec<u8> = (0..120_000u32).map(|i| (i.wrapping_mul(40_503) >> 7) as u8).collect();
        let attachment = upload_task_file(addr, &actor.session_token, source, &task, "복사 자료 📎.bin", &bytes).await;
        let (source_key, _) = file_rows(&run, source, attachment).await;
        let source_key = source_key.expect("source row");
        // The current body shows the file (product body write) and a manual
        // revision of the source keeps it; the copy carries only the body.
        let node = json!({"type":"attachment","attrs":{"id":attachment.to_string(),"name":"복사 자료 📎.bin"}});
        let shown = json!({"type":"doc","content":[para(&Uuid::now_v7().to_string(), "첨부를 보이는 본문"), node]});
        let (status, saved) = session_call(addr, Method::PUT, &document_api(source, document, "/body"), &actor.session_token, Some(json!({"contentJson":shown}))).await;
        assert!(status.is_success(), "{status}: {saved}");
        let (status, revision) = session_call(addr, Method::POST, &document_api(source, document, "/revisions"), &actor.session_token, None).await;
        assert_eq!(status, StatusCode::CREATED, "{revision}");
        selection_versions(&run, source, &mut selection).await;
        let mut copy = selection.clone();
        copy["action"] = json!("copy");
        let source_before = transfer_graph(&run, source).await;
        let (status, preview) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(copy.clone())).await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        assert_eq!(preview["attachmentCount"], 1);
        assert!(preview["dispositions"].as_array().unwrap().iter().any(|d| d["item"] == "attachment" && d["outcome"] == "copied_new_id" && d["count"] == 1), "{preview}");
        let body = json!({"requestId":Uuid::now_v7(),"selection":copy,"previewDigest":preview["digest"],"confirmed":true});
        let (status, copied) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body.clone())).await;
        assert_eq!(status, StatusCode::OK, "{copied}");
        let copied_document = Uuid::parse_str(copied["documentId"].as_str().unwrap()).unwrap();
        let copied_task = Uuid::parse_str(copied["taskId"].as_str().unwrap()).unwrap();
        assert_ne!(copied_document, document);
        assert_ne!(copied_task.to_string(), task);
        // Exactly one new attachment under the copied task, fresh key and id.
        let team_graph = transfer_graph(&run, team).await;
        let team_files = team_graph["attachments"].as_array().unwrap().clone();
        assert_eq!(team_files.len(), 1, "{team_files:?}");
        let new_file = Uuid::parse_str(team_files[0]["id"].as_str().unwrap()).unwrap();
        assert_ne!(new_file, attachment);
        assert_eq!(team_files[0]["task_id"], json!(copied_task.to_string()));
        assert_ne!(team_files[0]["storage_key"], json!(source_key));
        assert!(file_rows(&run, team, new_file).await.1.is_empty(), "staged journal consumed");
        // The copied body points at the new attachment, never the private one.
        let copied_body = rows_of(&team_graph, "documents", "id", copied_document, None)[0]["content_json"].to_string();
        assert!(copied_body.contains(&new_file.to_string()), "{copied_body}");
        assert!(!team_graph.to_string().contains(&attachment.to_string()), "no source attachment id in the team graph");
        assert!(!team_graph.to_string().contains(&document.to_string()), "no source document id in the team graph");
        let (status, downloaded) = download(addr, &actor.session_token, team, new_file).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(Sha256::digest(&downloaded), Sha256::digest(&bytes));
        // The private original is untouched and still downloadable.
        let source_after = transfer_graph(&run, source).await;
        for table in ["attachments", "documents", "revisions", "document_states", "document_collab_updates", "document_collab_op_receipts"] {
            assert_eq!(source_after[table], source_before[table], "{table}");
        }
        let (status, still) = download(addr, &actor.session_token, source, attachment).await;
        assert_eq!((status, still), (StatusCode::OK, bytes.clone()));
        // A lost response: the same request replays the stored receipt and
        // copies nothing again.
        let (status, replayed) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{replayed}");
        assert_eq!(replayed["replayed"], true);
        assert_eq!((replayed["documentId"].clone(), replayed["taskId"].clone()), (copied["documentId"].clone(), copied["taskId"].clone()));
        let team_again = transfer_graph(&run, team).await;
        assert_eq!(team_again["attachments"], team_graph["attachments"]);
        assert_eq!(team_again["documents"], team_graph["documents"]);
        run.finish().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn personal_transfer_copy_refuses_a_stored_image_node_before_seeding() {
    run_test(
        "personal_transfer_copy_refuses_a_stored_image_node_before_seeding",
        async {
            let mut run = TestRun::new(TestDb::bootstrap().await);
            let (addr, actor, source, _, _, mut selection) = setup_transfer(&mut run).await;
            let team = actor.workspace_id;
            let document = Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
            // Fixture: a stored (non-native) body with a legacy `image` node, which
            // the native seed schema cannot represent.
            let admin = admin_pool(&run.harness).await;
            sqlx::query("UPDATE fvoci.documents SET content_json = $2 WHERE id = $1")
                .bind(document)
                .bind(json!({"type":"doc","content":[{"type":"image","attrs":{"src":"x.png"}}]}))
                .execute(&admin)
                .await
                .unwrap();
            admin.close().await;
            selection_versions(&run, source, &mut selection).await;
            let mut copy = selection.clone();
            copy["action"] = json!("copy");
            let (source_before, team_before) = (
                transfer_graph(&run, source).await,
                transfer_graph(&run, team).await,
            );
            let (status, error) = session_call(
                addr,
                Method::POST,
                &format!("{}/preview", path(source)),
                &actor.session_token,
                Some(copy),
            )
            .await;
            assert_eq!(status, StatusCode::CONFLICT, "{error}");
            assert_eq!(error["params"]["code"], "body_encoding");
            assert_eq!(error["title"], "copy body node outside the native schema");
            assert_eq!(transfer_graph(&run, source).await, source_before);
            assert_eq!(transfer_graph(&run, team).await, team_before);
            run.finish().await.unwrap();
        },
    )
    .await;
}

#[tokio::test]
async fn personal_transfer_copy_refuses_truthy_non_array_content_or_marks_before_staging() {
    run_test("personal_transfer_copy_refuses_truthy_non_array_content_or_marks_before_staging", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, _, _, mut selection) = setup_transfer(&mut run).await;
        let team = actor.workspace_id;
        let document = Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
        let task = selection["taskId"].as_str().unwrap().to_string();
        // The pair has its own file, so an admitted COPY would stage it.
        let attachment = upload_task_file(addr, &actor.session_token, source, &task, "모양.bin", b"shape").await;
        let file = json!({"type":"attachment","attrs":{"id":attachment.to_string(),"name":"모양.bin"}});
        let store = |body: Value| {
            let harness = &run.harness;
            async move {
                let admin = admin_pool(harness).await;
                sqlx::query("UPDATE fvoci.documents SET content_json = $2 WHERE id = $1")
                    .bind(document)
                    .bind(body)
                    .execute(&admin)
                    .await
                    .unwrap();
                admin.close().await;
            }
        };
        let journal = || {
            let harness = &run.harness;
            async move {
                let admin = admin_pool(harness).await;
                let count: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.attachment_object_cleanups WHERE workspace_id = $1")
                    .bind(team)
                    .fetch_one(&admin)
                    .await
                    .unwrap();
                admin.close().await;
                count
            }
        };
        // The native seed itself (the real engine child) refuses truthy
        // non-array content and marks below the root, and accepts falsy ones.
        let seed = fvoci_server::collab::seed::SeedEngine::from_env().expect("FVOCI_COLLAB_ENGINE");
        let text = |marks: Value| json!({"type":"paragraph","content":[{"type":"text","text":"글","marks":marks}]});
        for bad in [
            json!({"type":"doc","content":[{"type":"paragraph","content":{"x":1}}]}),
            json!({"type":"doc","content":[text(json!({"x":1}))]}),
            json!({"type":"doc","content":[{"type":"paragraph","marks":{"x":1}}]}),
        ] {
            assert!(
                matches!(seed.tiptap_to_yjs_update(&bad).await, Err(fvoci_server::collab::seed::SeedError::InvalidInput(_))),
                "native seed accepted {bad}"
            );
        }
        for falsy in [Value::Null, json!(false), json!(0), json!("")] {
            let good = json!({"type":"doc","content":[{"type":"paragraph","content":falsy.clone()}, text(falsy)]});
            assert!(seed.tiptap_to_yjs_update(&good).await.is_ok(), "native seed refused {good}");
        }
        // Fixture: a stored body whose paragraph has truthy object content,
        // which the native seed refuses ("content is not an array").
        store(json!({"type":"doc","content":[file.clone(),{"type":"paragraph","content":{"x":1}}]})).await;
        selection_versions(&run, source, &mut selection).await;
        let mut copy = selection.clone();
        copy["action"] = json!("copy");
        let (source_before, team_before) = (transfer_graph(&run, source).await, transfer_graph(&run, team).await);
        let (status, error) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(copy.clone())).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["params"]["code"], "body_encoding");
        assert_eq!(error["title"], "copy body content is not an array");
        // The command path refuses the same way before staging (admission
        // reads the source graph before the digest): nothing journaled.
        let body = json!({"requestId":Uuid::now_v7(),"selection":copy,"previewDigest":"0".repeat(64),"confirmed":true});
        let (status, error) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["params"]["code"], "body_encoding");
        assert_eq!(error["title"], "copy body content is not an array");
        assert_eq!(journal().await, 0, "no staged key");
        assert_eq!(transfer_graph(&run, source).await, source_before);
        assert_eq!(transfer_graph(&run, team).await, team_before);
        // The same for truthy non-array marks on a text node ("marks is not
        // an array" in the seed).
        store(json!({"type":"doc","content":[file.clone(), text(json!({"x":1}))]})).await;
        selection_versions(&run, source, &mut selection).await;
        let mut copy = selection.clone();
        copy["action"] = json!("copy");
        let (source_before, team_before) = (transfer_graph(&run, source).await, transfer_graph(&run, team).await);
        let (status, error) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(copy.clone())).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["params"]["code"], "body_encoding");
        assert_eq!(error["title"], "copy body marks are not an array");
        let body = json!({"requestId":Uuid::now_v7(),"selection":copy,"previewDigest":"0".repeat(64),"confirmed":true});
        let (status, error) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["params"]["code"], "body_encoding");
        assert_eq!(error["title"], "copy body marks are not an array");
        assert_eq!(journal().await, 0, "no staged key");
        assert_eq!(transfer_graph(&run, source).await, source_before);
        assert_eq!(transfer_graph(&run, team).await, team_before);
        // Control: falsy content is no children and falsy marks no marks for
        // the seed, so the same pair copies with its file.
        let falsy = [Value::Null, json!(false), json!(0), json!("")].map(|content| json!({"type":"paragraph","content":content}));
        let mut nodes = vec![file];
        nodes.extend(falsy);
        nodes.extend([Value::Null, json!(false), json!(0), json!("")].map(text));
        store(json!({"type":"doc","content":nodes})).await;
        selection_versions(&run, source, &mut selection).await;
        let mut copy = selection.clone();
        copy["action"] = json!("copy");
        let (status, preview) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(copy.clone())).await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        let body = json!({"requestId":Uuid::now_v7(),"selection":copy,"previewDigest":preview["digest"],"confirmed":true});
        let (status, copied) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{copied}");
        let team_graph = transfer_graph(&run, team).await;
        let team_files = team_graph["attachments"].as_array().unwrap();
        assert_eq!(team_files.len(), 1, "{team_files:?}");
        assert_ne!(team_files[0]["id"], json!(attachment.to_string()));
        assert_eq!(journal().await, 0, "staged journal consumed");
        run.finish().await.unwrap();
    })
    .await;
}
