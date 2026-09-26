#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! Project lifecycle (DELETE/restore/archive) and project document
//! trash/restore/sort against the source contract, on the real app role.

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::http::StatusCode;
use fvoci_server::db::collab::{resolve_collab_admission, CollabDbError};
use project_harness::{
    add_workspace_user, admin_pool, app_pool, create_project, json_request, session_id_for_user,
    setup_session, TestDb,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

async fn create_doc(
    app: &axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    project_id: &str,
    parent_id: &str,
    title: &str,
) -> String {
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents"),
        Some(json!({"parentId": parent_id, "title": title})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    body["id"].as_str().unwrap().to_string()
}

async fn doc_state(admin: &PgPool, id: &str) -> (Option<String>, bool) {
    let row: (Option<Uuid>, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT parent_id, deleted_at FROM fvoci.documents WHERE id = $1")
            .bind(Uuid::parse_str(id).unwrap())
            .fetch_one(admin)
            .await
            .unwrap();
    (row.0.map(|id| id.to_string()), row.1.is_some())
}

async fn count_events(admin: &PgPool, workspace_id: Uuid, verb: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM fvoci.events WHERE workspace_id = $1 AND verb = $2")
        .bind(workspace_id)
        .bind(verb)
        .fetch_one(admin)
        .await
        .unwrap()
}

async fn count_audit(admin: &PgPool, workspace_id: Uuid, verb: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM fvoci.audit_log WHERE workspace_id = $1 AND verb = $2")
        .bind(workspace_id)
        .bind(verb)
        .fetch_one(admin)
        .await
        .unwrap()
}

fn trash_ids(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn project_document_trash_restore_sort_follow_source_contract() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, ws, "TRS", "private").await;
    let pid = project["id"].as_str().unwrap().to_string();
    let root = project["rootDocumentId"].as_str().unwrap().to_string();
    let base = format!("/api/v1/workspaces/{ws}/projects/{pid}/documents");
    let a = create_doc(&app, &cookie, ws, &pid, &root, "A").await;
    let a1 = create_doc(&app, &cookie, ws, &pid, &a, "A1").await;
    let b = create_doc(&app, &cookie, ws, &pid, &root, "B").await;

    // Sort: B first (afterId null), then the tree lists B before A.
    let (status, sorted) = json_request(
        app.clone(),
        "POST",
        &format!("{base}/{b}/sort"),
        Some(json!({"afterId": null})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sorted:?}");
    assert_eq!(sorted["displayId"], "TRS-4");
    let (_, tree) = json_request(app.clone(), "GET", &base, None, Some(&cookie)).await;
    let order: Vec<&str> = tree["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["parentId"] == root.as_str())
        .map(|node| node["id"].as_str().unwrap())
        .collect();
    assert_eq!(order, vec![b.as_str(), a.as_str()]);

    // The project root cannot be trashed.
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("{base}/{root}/trash"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["code"], "root_document_trash");

    // The wiki route pair does not reach project documents (affiliation).
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents/{a}/trash"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Reparent: A1 moves under the root, only A is trashed.
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("{base}/{a}/trash?children=reparent"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(doc_state(&admin, &a).await, (Some(root.clone()), true));
    assert_eq!(doc_state(&admin, &a1).await, (Some(root.clone()), false));

    // The workspace trash lists project documents with their project.
    let (_, trash) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/trash"),
        None,
        Some(&cookie),
    )
    .await;
    let item = trash["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == a.as_str())
        .expect("trashed project document listed");
    assert_eq!(item["projectId"], pid.as_str());

    // Wiki restore refuses the project document; the project route restores it.
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents/{a}/restore"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("{base}/{a}/restore"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert!(!doc_state(&admin, &a).await.1);

    // DELETE is the trash alias and takes the subtree by default.
    let a2 = create_doc(&app, &cookie, ws, &pid, &a, "A2").await;
    let (status, body) = json_request(
        app.clone(),
        "DELETE",
        &format!("{base}/{a}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert!(
        doc_state(&admin, &a2).await.1,
        "subtree trashed with parent"
    );
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("{base}/{a2}/restore"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body:?}");
    assert_eq!(body["code"], "restore_rejected");

    assert_eq!(count_events(&admin, ws, "document.trashed").await, 3);
    assert_eq!(count_audit(&admin, ws, "document.trashed").await, 3);
    assert_eq!(count_events(&admin, ws, "document.restored").await, 1);

    // Project viewer: can see the trash row, cannot trash or restore.
    let viewer = add_workspace_user(&admin, ws, "member", "viewer").await;
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{pid}/members"),
        Some(json!({"userId": viewer.user_id, "role": "viewer"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, viewer_trash) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/trash"),
        None,
        Some(&viewer.cookie),
    )
    .await;
    assert!(trash_ids(&viewer_trash).contains(&a));
    for path in [format!("{base}/{b}/trash"), format!("{base}/{a}/restore")] {
        let (status, _) =
            json_request(app.clone(), "POST", &path, None, Some(&viewer.cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }

    // Workspace member outside the private project: nothing visible.
    let outsider = add_workspace_user(&admin, ws, "member", "outsider").await;
    let (_, outsider_trash) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/trash"),
        None,
        Some(&outsider.cookie),
    )
    .await;
    assert!(!trash_ids(&outsider_trash).contains(&a));
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("{base}/{b}/sort"),
        Some(json!({"afterId": null})),
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn project_delete_restore_and_archive_follow_source_contract() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, ws, "LIFE", "workspace").await;
    let pid = project["id"].as_str().unwrap().to_string();
    let root = project["rootDocumentId"].as_str().unwrap().to_string();
    let project_url = format!("/api/v1/workspaces/{ws}/projects/{pid}");
    let kept = create_doc(&app, &cookie, ws, &pid, &root, "kept").await;
    let solo = create_doc(&app, &cookie, ws, &pid, &root, "trashed on its own").await;
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("{project_url}/documents/{solo}/trash"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Archive: read-only (writes refused, collab admission read-only), then back.
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("{project_url}/archive"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("{project_url}/documents"),
        Some(json!({"parentId": root, "title": "blocked"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let session_id = session_id_for_user(&admin, owner_id).await;
    let kept_id = Uuid::parse_str(&kept).unwrap();
    let admission = resolve_collab_admission(&pool, ws, owner_id, session_id, kept_id)
        .await
        .unwrap()
        .expect("archived project stays readable");
    assert!(admission.read_only && admission.archived);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("{project_url}/unarchive"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, fetched) = json_request(app.clone(), "GET", &project_url, None, Some(&cookie)).await;
    assert_eq!(fetched["status"], "active");

    // A workspace member without project manage cannot delete or archive.
    let member = add_workspace_user(&admin, ws, "member", "member").await;
    for (method, path) in [
        ("DELETE", project_url.clone()),
        ("POST", format!("{project_url}/archive")),
    ] {
        let (status, _) =
            json_request(app.clone(), method, &path, None, Some(&member.cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
    }

    // DELETE: project hidden, its live documents trashed with the project stamp.
    let (status, body) =
        json_request(app.clone(), "DELETE", &project_url, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let (status, _) = json_request(app.clone(), "GET", &project_url, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(doc_state(&admin, &kept).await.1);
    assert!(doc_state(&admin, &root).await.1);
    assert_eq!(
        resolve_collab_admission(&pool, ws, owner_id, session_id, kept_id)
            .await
            .unwrap(),
        Err(CollabDbError::NotFound)
    );
    let stamps: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT d.id FROM fvoci.documents d
        JOIN fvoci.projects p ON p.id = d.project_id
        WHERE p.id = $1 AND d.deleted_at = p.deleted_at
        "#,
    )
    .bind(Uuid::parse_str(&pid).unwrap())
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(stamps.len(), 2, "root and kept share the project stamp");

    // Deleted list and restore are workspace-admin only.
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects?deleted=true"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("{project_url}/restore"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, deleted) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects?deleted=true"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(trash_ids(&deleted), vec![pid.clone()]);

    let (status, restored) = json_request(
        app.clone(),
        "POST",
        &format!("{project_url}/restore"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{restored:?}");
    assert_eq!(restored["id"], pid.as_str());
    assert!(!doc_state(&admin, &kept).await.1);
    assert!(!doc_state(&admin, &root).await.1);
    assert!(
        doc_state(&admin, &solo).await.1,
        "a document trashed on its own stays in the trash"
    );
    for verb in [
        "project.archived",
        "project.unarchived",
        "project.deleted",
        "project.restored",
    ] {
        assert_eq!(count_events(&admin, ws, verb).await, 1, "{verb}");
        assert_eq!(count_audit(&admin, ws, verb).await, 1, "{verb}");
    }

    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn trash_rows_past_retention_are_not_restorable() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, ws, "OLD", "workspace").await;
    let pid = project["id"].as_str().unwrap().to_string();
    let root = project["rootDocumentId"].as_str().unwrap().to_string();
    let doc = create_doc(&app, &cookie, ws, &pid, &root, "old").await;
    let (status, wiki) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents"),
        Some(json!({"title": "old wiki", "parentId": null})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let wiki_id = wiki["id"].as_str().unwrap().to_string();
    for path in [
        format!("/api/v1/workspaces/{ws}/projects/{pid}/documents/{doc}/trash"),
        format!("/api/v1/workspaces/{ws}/documents/{wiki_id}/trash"),
    ] {
        let (status, _) = json_request(app.clone(), "POST", &path, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK);
    }
    sqlx::query(
        "UPDATE fvoci.documents SET deleted_at = now() - interval '31 days' WHERE id = ANY($1)",
    )
    .bind(vec![
        Uuid::parse_str(&doc).unwrap(),
        Uuid::parse_str(&wiki_id).unwrap(),
    ])
    .execute(&admin)
    .await
    .unwrap();
    let (_, trash) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/trash"),
        None,
        Some(&cookie),
    )
    .await;
    assert!(trash_ids(&trash).is_empty(), "{trash:?}");
    for path in [
        format!("/api/v1/workspaces/{ws}/projects/{pid}/documents/{doc}/restore"),
        format!("/api/v1/workspaces/{ws}/documents/{wiki_id}/restore"),
    ] {
        let (status, _) = json_request(app.clone(), "POST", &path, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{ws}/projects/{pid}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    sqlx::query("UPDATE fvoci.projects SET deleted_at = now() - interval '31 days' WHERE id = $1")
        .bind(Uuid::parse_str(&pid).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    let (_, deleted) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects?deleted=true"),
        None,
        Some(&cookie),
    )
    .await;
    assert!(trash_ids(&deleted).is_empty());
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{pid}/restore"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

/// Project DELETE and a concurrent project document create serialize on the
/// workspace tree lock: whichever order commits, no live document is left in
/// a deleted project.
#[tokio::test]
async fn project_delete_and_concurrent_document_create_leave_no_live_orphan() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, ws, "RACE", "workspace").await;
    // A second user, so the two requests do not serialize on one actor lock.
    let writer = add_workspace_user(&admin, ws, "member", "writer").await;
    let pid = project["id"].as_str().unwrap().to_string();
    let root = project["rootDocumentId"].as_str().unwrap().to_string();

    // Hold the tree lock so both requests queue behind it, then release.
    let mut holder = admin.begin().await.unwrap();
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(fvoci_server::db::context::TREE_LOCK_NAMESPACE)
        .bind(fvoci_server::db::context::lock_key_from_uuid(ws))
        .execute(&mut *holder)
        .await
        .unwrap();
    let delete = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
        let url = format!("/api/v1/workspaces/{ws}/projects/{pid}");
        async move { json_request(app, "DELETE", &url, None, Some(&cookie)).await }
    });
    let create = tokio::spawn({
        let app = app.clone();
        let cookie = writer.cookie.clone();
        let url = format!("/api/v1/workspaces/{ws}/projects/{pid}/documents");
        let root = root.clone();
        async move {
            json_request(
                app,
                "POST",
                &url,
                Some(json!({"parentId": root, "title": "racing"})),
                Some(&cookie),
            )
            .await
        }
    });
    project_harness::wait_for_blocked_query_count(&admin, holder_pid, "%pg_advisory_xact_lock%", 2)
        .await;
    holder.commit().await.unwrap();
    let (delete_status, _) = delete.await.unwrap();
    let (create_status, _) = create.await.unwrap();
    assert_eq!(delete_status, StatusCode::OK);
    assert!(
        create_status == StatusCode::CREATED || create_status == StatusCode::NOT_FOUND,
        "{create_status}"
    );
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.documents WHERE project_id = $1 AND deleted_at IS NULL",
    )
    .bind(Uuid::parse_str(&pid).unwrap())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(live, 0, "no live document may remain in a deleted project");

    admin.close().await;
    harness.cleanup().await;
}
