//! Actual preview input acquisition against OFF body saving. Both callers use
//! restricted app roles; a native-state barrier proves the conflicting order.
use super::*;
use fvoci_server::db::attachment_preview::{claim_preview, load_preview_input};

#[tokio::test]
async fn document_save_and_preview_input_do_not_deadlock() {
    off_body_save_and_preview_input(fvoci_server::db::collab::CollabKind::Document).await;
}

#[tokio::test]
async fn task_save_and_preview_input_do_not_deadlock() {
    off_body_save_and_preview_input(fvoci_server::db::collab::CollabKind::Task).await;
}

#[tokio::test]
async fn workspace_deletion_wait_is_rechecked_before_preview_publication() {
    use fvoci_server::db::attachment_preview::{journal_preview_key, publish_preview};

    let harness = TestDb::bootstrap().await;
    let app = pool::connect_app_with_max(&harness.app_url, 1)
        .await
        .unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let (workspace, actor, document) = seed_workspace(&admin).await;
    let attachment =
        insert_pending_attachment(&admin, workspace, document, actor, "delete.png").await;
    sqlx::query("UPDATE fvoci.attachments SET image=true,mime='image/png',preview_status='pending' WHERE id=$1")
        .bind(attachment).execute(&admin).await.unwrap();
    let claim = claim_preview(&app).await.unwrap().unwrap();
    let key = Uuid::now_v7().to_string();
    let journal = journal_preview_key(&app, &claim, &key).await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&app)
        .await
        .unwrap();
    let mut deletion = admin.begin().await.unwrap();
    let deletion_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *deletion)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.workspaces SET deleted_at=now() WHERE id=$1")
        .bind(workspace)
        .execute(&mut *deletion)
        .await
        .unwrap();
    let publish_app = app.clone();
    let owner = claim.clone();
    let publish = tokio::spawn(async move {
        publish_preview(&publish_app, &owner, journal, &key, 1, 1, 16).await
    });
    wait_blocked_by(&admin, pid, deletion_pid).await;
    deletion.commit().await.unwrap();
    let result = publish.await.unwrap();
    let state: (String, Option<Uuid>, bool, i64) = sqlx::query_as(
        "SELECT preview_status,preview_lease_token,variants ? 'preview',
         (SELECT count(*) FROM fvoci.attachment_object_cleanups WHERE id=$2)
         FROM fvoci.attachments WHERE id=$1",
    )
    .bind(attachment)
    .bind(journal)
    .fetch_one(&admin)
    .await
    .unwrap();
    app.close().await;
    admin.close().await;
    harness.cleanup().await;
    assert!(!result.unwrap());
    assert_eq!(state, ("pending".into(), Some(claim.lease_token), false, 1));
}

async fn off_body_save_and_preview_input(kind: fvoci_server::db::collab::CollabKind) {
    use fvoci_server::collab::{seed::SeedEngine, CollabConfig};
    use fvoci_server::config::RealtimeMode;
    use fvoci_server::db::backend::Backend;
    use fvoci_server::db::body_save::{save_off_body, OffBodyRequest};
    use fvoci_server::db::collab::{claim_writer_and_load_kind, CollabKind};
    use fvoci_server::db::revisions::RevisionTarget;

    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let append_app = pool::connect_app_with_max(&harness.app_url, 1)
        .await
        .unwrap();
    let finish_app = pool::connect_app_with_max(&harness.app_url, 1)
        .await
        .unwrap();
    let flags: (bool, bool) =
        sqlx::query_as("SELECT rolsuper,rolbypassrls FROM pg_roles WHERE rolname=current_user")
            .fetch_one(&append_app)
            .await
            .unwrap();
    assert_eq!(flags, (false, false));
    let (workspace, actor, document) = seed_workspace(&admin).await;
    sqlx::query("UPDATE fvoci.documents SET content_json=$2 WHERE id=$1")
        .bind(document)
        .bind(fvoci_server::db::documents::empty_document_json())
        .execute(&admin)
        .await
        .unwrap();
    let credential = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.sessions(id,user_id,token_hash,expires_at) VALUES($1,$2,$3,now()+interval '1 day')")
        .bind(credential).bind(actor).bind(Uuid::now_v7().to_string())
        .execute(&admin).await.unwrap();
    let resource = match kind {
        CollabKind::Document => document,
        CollabKind::Task => {
            let project = Uuid::now_v7();
            let workflow = Uuid::now_v7();
            let status = Uuid::now_v7();
            let task = Uuid::now_v7();
            sqlx::query("INSERT INTO fvoci.projects(id,workspace_id,key,name,visibility,created_by) VALUES($1,$2,'RACE','Race','workspace',$3)")
                .bind(project).bind(workspace).bind(actor).execute(&admin).await.unwrap();
            sqlx::query("INSERT INTO fvoci.workflows(id,workspace_id,project_id) VALUES($1,$2,$3)")
                .bind(workflow)
                .bind(workspace)
                .bind(project)
                .execute(&admin)
                .await
                .unwrap();
            sqlx::query("INSERT INTO fvoci.statuses(id,workspace_id,project_id,workflow_id,name,category,sort_key) VALUES($1,$2,$3,$4,'Todo','todo','V')")
                .bind(status).bind(workspace).bind(project).bind(workflow).execute(&admin).await.unwrap();
            sqlx::query("INSERT INTO fvoci.tasks(id,workspace_id,project_id,status_id,number,title,content_json,created_by) VALUES($1,$2,$3,$4,1,'Task',$5,$6)")
                .bind(task).bind(workspace).bind(project).bind(status)
                .bind(fvoci_server::db::documents::empty_document_json()).bind(actor).execute(&admin).await.unwrap();
            task
        }
    };
    let (state_table, receipt_table, id_column, target) = match kind {
        CollabKind::Document => (
            "document_states",
            "document_collab_op_receipts",
            "document_id",
            RevisionTarget::Document(resource),
        ),
        CollabKind::Task => (
            "task_states",
            "task_collab_op_receipts",
            "task_id",
            RevisionTarget::Task(resource),
        ),
    };
    claim_writer_and_load_kind(&append_app, kind, workspace, actor, credential, resource)
        .await
        .unwrap()
        .unwrap();
    let attachment =
        insert_pending_attachment(&admin, workspace, document, actor, "race.hwp").await;
    if kind == CollabKind::Task {
        sqlx::query("UPDATE fvoci.attachments SET document_id=NULL,task_id=$2 WHERE id=$1")
            .bind(attachment)
            .bind(resource)
            .execute(&admin)
            .await
            .unwrap();
    }
    let engine = CollabConfig::from_env().expect("current owned collab engine required");
    let update = SeedEngine::new(engine.engine_bin.clone(), engine.limits)
        .tiptap_to_yjs_update(&serde_json::json!({"type":"doc","content":[
            {"type":"paragraph","attrs":{"id":"saved-block"},"content":[{"type":"text","text":"saved body"}]}
        ]}))
        .await.unwrap();
    sqlx::query("UPDATE fvoci.attachments SET image=true,mime='image/png',preview_status='pending' WHERE id=$1")
        .bind(attachment).execute(&admin).await.unwrap();
    let claim = claim_preview(&finish_app).await.unwrap().unwrap();
    assert_eq!(claim.attachment_id, attachment);
    let append_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&append_app)
        .await
        .unwrap();
    let finish_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&finish_app)
        .await
        .unwrap();
    let mut barrier = admin.begin().await.unwrap();
    let barrier_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!("SELECT {id_column} FROM fvoci.{state_table} WHERE workspace_id=$1 AND {id_column}=$2 FOR UPDATE")))
        .bind(workspace).bind(resource).fetch_one(&mut *barrier).await.unwrap();
    let operation = Uuid::now_v7();
    let append_pool = append_app.clone();
    let append = tokio::spawn(async move {
        save_off_body(
            &Backend::Postgres(append_pool),
            RealtimeMode::Off,
            engine,
            OffBodyRequest {
                workspace,
                actor,
                credential,
                target,
                project: None,
                expected_tail: 0,
                command: operation,
                update,
                client_ip: None,
            },
        )
        .await
    });
    wait_blocked_by(&admin, append_pid, barrier_pid).await;
    let finish_pool = finish_app.clone();
    let finish = tokio::spawn(async move { load_preview_input(&finish_pool, &claim).await });
    wait_blocked_by(&admin, finish_pid, append_pid).await;
    barrier.commit().await.unwrap();
    let append_result = append.await.unwrap();
    let finish_result = finish.await.unwrap();

    // OFF saves compact their update into the snapshot; the immutable native
    // receipt and state head, rather than an uncompacted tail, prove the ACK.
    let native_count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM fvoci.{receipt_table} WHERE workspace_id=$1 AND {id_column}=$2 AND op_id=$3")))
        .bind(workspace).bind(resource).bind(operation).fetch_one(&admin).await.unwrap();
    let native_head: (i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT tail_seq,snapshot_cutoff_seq FROM fvoci.{state_table} WHERE workspace_id=$1 AND {id_column}=$2")))
        .bind(workspace).bind(resource).fetch_one(&admin).await.unwrap();
    let committed: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM fvoci.body_save_commands WHERE command_id=$1),(SELECT count(*) FROM fvoci.revisions WHERE target_id=$2),(SELECT count(*) FROM fvoci.attachment_text WHERE attachment_id=$3),(SELECT count(*) FROM fvoci.events WHERE target_id=$3 AND verb='attachment.extracted')")
        .bind(operation).bind(resource).bind(attachment).fetch_one(&admin).await.unwrap();
    let resource_table = if kind == CollabKind::Document {
        "documents"
    } else {
        "tasks"
    };
    let projected: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT content_json FROM fvoci.{resource_table} WHERE workspace_id=$1 AND id=$2"
    )))
    .bind(workspace)
    .bind(resource)
    .fetch_one(&admin)
    .await
    .unwrap();
    let revision_body: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT content_json FROM fvoci.revisions WHERE workspace_id=$1 AND target_id=$2",
    )
    .bind(workspace)
    .bind(resource)
    .fetch_optional(&admin)
    .await
    .unwrap();
    let snapshot: Vec<u8> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT state FROM fvoci.{state_table} WHERE workspace_id=$1 AND {id_column}=$2"
    )))
    .bind(workspace)
    .bind(resource)
    .fetch_one(&admin)
    .await
    .unwrap();
    let snapshot_engine = CollabConfig::from_env().unwrap();
    let decoded = fvoci_server::collab::revision::capture_revision_offline(
        snapshot_engine.engine_bin,
        snapshot_engine.limits,
        snapshot,
        vec![],
    );
    append_app.close().await;
    finish_app.close().await;
    admin.close().await;
    harness.cleanup().await;
    assert_eq!(append_result.unwrap().tail_seq, "1");
    let preview = finish_result
        .unwrap()
        .expect("live current claim must still load");
    assert_eq!(preview.mime, "image/png");
    assert_eq!(preview.size_bytes, 16);
    assert_eq!(native_count, 1);
    assert_eq!(Some(&projected), revision_body.as_ref());
    assert_eq!(projected, decoded.unwrap().content_json);
    assert_eq!(projected["content"][0]["content"][0]["text"], "saved body");
    assert_eq!(projected["content"][0]["attrs"]["id"], "saved-block");
    assert_eq!(native_head, (1, 1));
    assert_eq!(committed, (1, 1, 0, 0));
}
