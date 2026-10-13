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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "title": "old wiki", "parentId": null})),
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

/// SQLite family ports of the project routes: every operation runs through
/// the real router on a migrated database and is read back on a new
/// connection.
mod sqlite_port {
    use super::project_harness::{http_request, json_request};
    use axum::http::StatusCode;
    use fvoci_server::auth::{password::Keyring, AuthService};
    use fvoci_server::db::{backend::Backend, migrate, pool, Db};
    use fvoci_server::http::{rate_limit::RateLimiter, state::AppState};
    use serde_json::{json, Value};
    use sqlx::SqlitePool;
    use std::{path::PathBuf, sync::Arc};
    use uuid::Uuid;

    pub(super) fn blob(id: Uuid) -> Vec<u8> {
        id.as_bytes().to_vec()
    }

    pub(super) fn id_of(value: &Value) -> Uuid {
        Uuid::parse_str(value.as_str().expect("id string")).expect("uuid")
    }

    pub(super) struct Member {
        pub id: Uuid,
        pub cookie: String,
    }

    pub(super) struct Lite {
        root: PathBuf,
        pub path: PathBuf,
        pub pool: SqlitePool,
        pub app: axum::Router,
        pub ws: Uuid,
        pub owner: Member,
    }

    fn app(backend: Backend, root: &std::path::Path) -> axum::Router {
        fvoci_server::http::router(
            AppState {
                realtime_mode: fvoci_server::config::RealtimeMode::On,
                native_engine: None,
                auth: Arc::new(AuthService {
                    db: Db::from_backend(backend),
                    password_keys: Keyring::parse(
                        r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
                        "test",
                    )
                    .unwrap(),
                }),
                branding_name: "Projects".into(),
                public_origin: "http://localhost".into(),
                cookie_secure: false,
                rate_limiter: RateLimiter::new(),
                storage: fvoci_server::attachments::LocalStorage::new(root.join("storage")).into(),
                upload: fvoci_server::attachments::UploadLimits {
                    part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
                    max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
                    create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
                    part_put_slots: fvoci_server::attachments::PartPutSlots::new(
                        fvoci_server::config::DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS,
                    ),
                },
                collab: None,
                meili: None,
                search_embedder: None,
                markdown: None,
                import_wake: None,
                import_extractor_available: false,
                preview_extract: None,
                quota: Default::default(),
                mailer: Arc::new(fvoci_server::mail::Mailer::disabled()),
                streams: AppState::fresh_streams(),
            },
            None,
        )
    }

    impl Lite {
        pub async fn new() -> Self {
            let root = std::env::temp_dir().join(format!("projects-port-{}", Uuid::now_v7()));
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join("projects.sqlite");
            migrate::run_sqlite_migrations(&path).await.unwrap();
            let pool = pool::connect_sqlite_app(&path, 4).await.unwrap();
            let app = app(Backend::Sqlite(pool.clone()), &root);
            let ws = insert_workspace(&pool, "lite").await;
            let owner = insert_member(&pool, ws, "owner", "owner").await;
            Self {
                root,
                path,
                pool,
                app,
                ws,
                owner,
            }
        }

        pub async fn workspace(&self, slug: &str) -> Uuid {
            insert_workspace(&self.pool, slug).await
        }

        pub async fn member(&self, ws: Uuid, role: &str, label: &str) -> Member {
            insert_member(&self.pool, ws, role, label).await
        }

        /// A personal access token of `who` in this workspace with `scopes`.
        pub async fn token(&self, who: &Member, scopes: &str) -> String {
            let token = fvoci_server::auth::token::new_token();
            sqlx::query("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes) VALUES(?1,?2,?3,?4,'Port',?5)")
                .bind(blob(Uuid::now_v7()))
                .bind(blob(self.ws))
                .bind(blob(who.id))
                .bind(&token.hash)
                .bind(scopes)
                .execute(&self.pool)
                .await
                .unwrap();
            token.token
        }

        pub fn url(&self, rest: &str) -> String {
            format!("/api/v1/workspaces/{}{rest}", self.ws)
        }

        pub async fn call(
            &self,
            method: &str,
            rest: &str,
            body: Option<Value>,
            who: &Member,
        ) -> (StatusCode, Value) {
            json_request(
                self.app.clone(),
                method,
                &self.url(rest),
                body,
                Some(&who.cookie),
            )
            .await
        }

        pub async fn bearer(
            &self,
            method: &str,
            rest: &str,
            body: Option<Value>,
            token: &str,
        ) -> (StatusCode, Value) {
            let header = format!("Bearer {token}");
            let (status, body, _) = http_request(
                self.app.clone(),
                method,
                &self.url(rest),
                body.map(|body| body.to_string().into_bytes()),
                Some("application/json"),
                None,
                &[("authorization", header.as_str())],
            )
            .await;
            (status, body)
        }

        /// A new connection: readback never reuses the writer's pool.
        pub async fn fresh(&self) -> SqlitePool {
            pool::connect_sqlite_app(&self.path, 1).await.unwrap()
        }

        pub async fn project(&self, key: &str, visibility: &str) -> (Uuid, Uuid) {
            let (status, body) = self
                .call(
                    "POST",
                    "/projects",
                    Some(json!({"key": key, "name": key, "visibility": visibility})),
                    &self.owner,
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            (id_of(&body["id"]), id_of(&body["rootDocumentId"]))
        }

        pub async fn finish(self) {
            self.pool.close().await;
            drop(self.app);
            std::fs::remove_dir_all(self.root).unwrap();
        }
    }

    async fn insert_workspace(pool: &SqlitePool, slug: &str) -> Uuid {
        let ws = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,?2,?2)")
            .bind(blob(ws))
            .bind(slug)
            .execute(pool)
            .await
            .unwrap();
        ws
    }

    async fn insert_member(pool: &SqlitePool, ws: Uuid, role: &str, label: &str) -> Member {
        let id = Uuid::now_v7();
        let token = fvoci_server::auth::token::new_token();
        sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,?2,?3)")
            .bind(blob(id))
            .bind(format!("{label}@example.test"))
            .bind(label)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,?3)")
            .bind(blob(ws))
            .bind(blob(id))
            .bind(role)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,(unixepoch()+3600)*1000000)")
            .bind(blob(Uuid::now_v7()))
            .bind(blob(id))
            .bind(&token.hash)
            .execute(pool)
            .await
            .unwrap();
        Member {
            id,
            cookie: token.token,
        }
    }

    pub(super) async fn scalar(pool: &SqlitePool, sql: &str, ids: &[Uuid]) -> i64 {
        let mut query = sqlx::query_scalar::<_, i64>(sql);
        for id in ids {
            query = query.bind(blob(*id));
        }
        query.fetch_one(pool).await.unwrap()
    }

    pub(super) async fn text(pool: &SqlitePool, sql: &str, ids: &[Uuid]) -> Option<String> {
        let mut query = sqlx::query_scalar::<_, Option<String>>(sql);
        for id in ids {
            query = query.bind(blob(*id));
        }
        query.fetch_one(pool).await.unwrap()
    }

    /// `events` and `audit_log` rows of `verb` in the workspace.
    pub(super) async fn journal(pool: &SqlitePool, ws: Uuid, verb: &str) -> (i64, i64) {
        sqlx::query_as(
            "SELECT (SELECT count(*) FROM events WHERE workspace_id=?1 AND verb=?2),
                    (SELECT count(*) FROM audit_log WHERE workspace_id=?1 AND verb=?2)",
        )
        .bind(blob(ws))
        .bind(verb)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    pub(super) async fn member_roles(pool: &SqlitePool, project: Uuid) -> Vec<(Uuid, String)> {
        sqlx::query_as::<_, (Vec<u8>, String)>(
            "SELECT user_id,role FROM project_members WHERE project_id=?1 AND user_id IS NOT NULL ORDER BY user_id",
        )
        .bind(blob(project))
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|(id, role)| (Uuid::from_slice(&id).unwrap(), role))
        .collect()
    }
}

async fn sqlite_document(
    lite: &sqlite_port::Lite,
    project: Uuid,
    parent: Uuid,
    title: &str,
) -> Uuid {
    let (status, body) = lite
        .call(
            "POST",
            &format!("/projects/{project}/documents"),
            Some(json!({"parentId": parent, "title": title})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    sqlite_port::id_of(&body["id"])
}

/// `deleted_at` of each id (None: live), on the given connection.
async fn sqlite_stamps(pool: &sqlx::SqlitePool, table: &str, ids: &[Uuid]) -> Vec<Option<i64>> {
    let sql = format!("SELECT deleted_at FROM {table} WHERE id=?1");
    let mut out = Vec::new();
    for id in ids {
        out.push(
            sqlx::query_scalar(&sql)
                .bind(sqlite_port::blob(*id))
                .fetch_one(pool)
                .await
                .unwrap(),
        );
    }
    out
}

/// Archive, trash (DELETE), the deleted list and restore on SQLite: one
/// stamp marks the project and its live subtree, restore brings back exactly
/// those rows, and retention and access refusals change nothing.
#[tokio::test]
async fn sqlite_project_lifecycle_keeps_the_postgres_contract() {
    use sqlite_port::{blob, journal, scalar, text, Lite};

    let lite = Lite::new().await;
    let (project, root) = lite.project("LIFE", "workspace").await;
    let p = format!("/projects/{project}");
    let a = sqlite_document(&lite, project, root, "A").await;
    let b = sqlite_document(&lite, project, a, "B").await;
    let earlier = sqlite_document(&lite, project, root, "Earlier").await;
    let (status, task) = lite
        .call(
            "POST",
            &format!("{p}/tasks"),
            Some(json!({"title": "Kept"})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let member = lite.member(lite.ws, "member", "member").await;
    let stranger_ws = lite.workspace("strangers").await;
    let stranger = lite.member(stranger_ws, "owner", "stranger").await;

    // Archive and unarchive need Manage; an archived project refuses edits.
    for who in [&member, &stranger] {
        let (status, _) = lite.call("POST", &format!("{p}/archive"), None, who).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, _) = lite
        .call("POST", &format!("{p}/archive"), None, &lite.owner)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = lite
        .call("PATCH", &p, Some(json!({"name": "Frozen"})), &lite.owner)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "project_archived");
    let fresh = lite.fresh().await;
    assert_eq!(
        text(
            &fresh,
            "SELECT status FROM projects WHERE id=?1",
            &[project]
        )
        .await
        .as_deref(),
        Some("archived")
    );
    assert_eq!(journal(&fresh, lite.ws, "project.archived").await, (1, 1));
    fresh.close().await;
    let (status, _) = lite
        .call("POST", &format!("{p}/unarchive"), None, &lite.owner)
        .await;
    assert_eq!(status, StatusCode::OK);
    let fresh = lite.fresh().await;
    assert_eq!(
        text(
            &fresh,
            "SELECT status||'|'||name FROM projects WHERE id=?1",
            &[project]
        )
        .await
        .as_deref(),
        Some("active|LIFE")
    );
    assert_eq!(journal(&fresh, lite.ws, "project.unarchived").await, (1, 1));
    fresh.close().await;

    // A document trashed on its own keeps its own stamp through the project's.
    let (status, _) = lite
        .call(
            "POST",
            &format!("{p}/documents/{earlier}/trash"),
            None,
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    sqlx::query("UPDATE documents SET deleted_at=deleted_at-1000000 WHERE id=?1")
        .bind(blob(earlier))
        .execute(&lite.pool)
        .await
        .unwrap();
    for who in [&member, &stranger] {
        let (status, _) = lite.call("DELETE", &p, None, who).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let fresh = lite.fresh().await;
    let before = sqlite_stamps(&fresh, "documents", &[earlier]).await;
    fresh.close().await;
    let (status, _) = lite.call("DELETE", &p, None, &lite.owner).await;
    assert_eq!(status, StatusCode::OK);
    let fresh = lite.fresh().await;
    let stamp = sqlite_stamps(&fresh, "projects", &[project]).await[0];
    assert!(stamp.is_some());
    assert_eq!(
        sqlite_stamps(&fresh, "documents", &[root, a, b]).await,
        vec![stamp; 3]
    );
    assert_eq!(sqlite_stamps(&fresh, "documents", &[earlier]).await, before);
    assert_eq!(journal(&fresh, lite.ws, "document.trashed").await, (4, 4));
    assert_eq!(journal(&fresh, lite.ws, "project.deleted").await, (1, 1));
    fresh.close().await;

    let (status, active) = lite.call("GET", "/projects", None, &lite.owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(active["items"], json!([]));
    let (status, deleted) = lite
        .call("GET", "/projects?deleted=true", None, &lite.owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    assert_eq!(deleted["items"][0]["id"], project.to_string());
    assert_eq!(deleted["items"].as_array().unwrap().len(), 1);
    for who in [&member, &stranger] {
        let (status, _) = lite.call("GET", "/projects?deleted=true", None, who).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = lite.call("POST", &format!("{p}/restore"), None, who).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    let (status, body) = lite
        .call("POST", &format!("{p}/restore"), None, &lite.owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], project.to_string());
    assert_eq!(body["status"], "active");
    let fresh = lite.fresh().await;
    assert_eq!(
        sqlite_stamps(&fresh, "projects", &[project]).await,
        vec![None]
    );
    assert_eq!(
        sqlite_stamps(&fresh, "documents", &[root, a, b]).await,
        vec![None; 3]
    );
    assert_eq!(sqlite_stamps(&fresh, "documents", &[earlier]).await, before);
    assert_eq!(journal(&fresh, lite.ws, "document.restored").await, (3, 3));
    assert_eq!(journal(&fresh, lite.ws, "project.restored").await, (1, 1));
    assert_eq!(
        scalar(
            &fresh,
            "SELECT count(*) FROM collection_items i JOIN collections c ON c.id=i.collection_id WHERE c.project_id=?1 AND c.kind='task'",
            &[project]
        )
        .await,
        1
    );
    fresh.close().await;

    // Past retention (by the DB clock) the project is neither listed nor
    // restorable.
    let (status, _) = lite.call("DELETE", &p, None, &lite.owner).await;
    assert_eq!(status, StatusCode::OK);
    sqlx::query("UPDATE projects SET deleted_at=deleted_at-31*86400000000 WHERE id=?1")
        .bind(blob(project))
        .execute(&lite.pool)
        .await
        .unwrap();
    let (status, deleted) = lite
        .call("GET", "/projects?deleted=true", None, &lite.owner)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted["items"], json!([]));
    let (status, _) = lite
        .call("POST", &format!("{p}/restore"), None, &lite.owner)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let fresh = lite.fresh().await;
    assert!(sqlite_stamps(&fresh, "projects", &[project]).await[0].is_some());
    assert_eq!(journal(&fresh, lite.ws, "project.restored").await, (1, 1));
    fresh.close().await;
    lite.finish().await;
}

/// Project document meta, sort, trash and restore on SQLite.
#[tokio::test]
async fn sqlite_project_documents_keep_the_postgres_contract() {
    use sqlite_port::{blob, journal, scalar, text, Lite};

    let lite = Lite::new().await;
    let (project, root) = lite.project("DOCS", "private").await;
    let d = |doc: Uuid| format!("/projects/{project}/documents/{doc}");
    let a = sqlite_document(&lite, project, root, "A").await;
    let b = sqlite_document(&lite, project, root, "B").await;
    let c = sqlite_document(&lite, project, root, "C").await;
    let child = sqlite_document(&lite, project, a, "Child").await;
    let viewer = lite.member(lite.ws, "member", "viewer").await;
    let (status, _) = lite
        .call(
            "POST",
            &format!("/projects/{project}/members"),
            Some(json!({"userId": viewer.id, "role": "viewer"})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let outsider = lite.member(lite.ws, "member", "outsider").await;

    let (status, body) = lite
        .call("PATCH", &d(a), Some(json!({"title": "Alpha"})), &lite.owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["title"], "Alpha");
    assert_eq!(body["displayId"], "DOCS-2");
    for (who, title, expected) in [
        (&lite.owner, "Bad\u{0}title", StatusCode::BAD_REQUEST),
        (&viewer, "Viewer", StatusCode::NOT_FOUND),
        (&outsider, "Outsider", StatusCode::NOT_FOUND),
    ] {
        let (status, _) = lite
            .call("PATCH", &d(a), Some(json!({"title": title})), who)
            .await;
        assert_eq!(status, expected, "{title:?}");
    }
    let fresh = lite.fresh().await;
    assert_eq!(
        text(&fresh, "SELECT title FROM documents WHERE id=?1", &[a])
            .await
            .as_deref(),
        Some("Alpha")
    );
    fresh.close().await;

    // Sort: first, then after a sibling; a non-sibling anchor is not found.
    let sort = |doc| format!("{}/sort", d(doc));
    let (status, body) = lite
        .call(
            "POST",
            &sort(c),
            Some(json!({"afterId": null})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let order = || async {
        let fresh = lite.fresh().await;
        let rows: Vec<Vec<u8>> = sqlx::query_scalar(
            "SELECT id FROM documents WHERE parent_id=?1 AND deleted_at IS NULL ORDER BY sort_key COLLATE BINARY",
        )
        .bind(blob(root))
        .fetch_all(&fresh)
        .await
        .unwrap();
        fresh.close().await;
        rows.into_iter()
            .map(|id| Uuid::from_slice(&id).unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(order().await, [c, a, b]);
    let (status, _) = lite
        .call("POST", &sort(c), Some(json!({"afterId": a})), &lite.owner)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(order().await, [a, c, b]);
    for (who, after, expected) in [
        (&lite.owner, Some(child), StatusCode::NOT_FOUND),
        (&lite.owner, Some(c), StatusCode::NOT_FOUND),
        (&viewer, None, StatusCode::NOT_FOUND),
        (&outsider, None, StatusCode::NOT_FOUND),
    ] {
        let (status, _) = lite
            .call("POST", &sort(c), Some(json!({"afterId": after})), who)
            .await;
        assert_eq!(status, expected);
    }
    assert_eq!(order().await, [a, c, b]);
    let fresh = lite.fresh().await;
    let reorders = scalar(
        &fresh,
        "SELECT count(*) FROM events WHERE verb='document.moved' AND json_extract(payload,'$.kind')='reorder' AND target_id=?1",
        &[c],
    )
    .await;
    assert_eq!(reorders, 2);
    fresh.close().await;

    // Trash and restore: a child waits for its parent; restore is per row.
    let (status, _) = lite
        .call("POST", &format!("{}/trash", d(a)), None, &lite.owner)
        .await;
    assert_eq!(status, StatusCode::OK);
    for (who, doc, expected, code) in [
        (&viewer, a, StatusCode::NOT_FOUND, None),
        (&outsider, a, StatusCode::NOT_FOUND, None),
        (
            &lite.owner,
            child,
            StatusCode::CONFLICT,
            Some("restore_rejected"),
        ),
        (&lite.owner, b, StatusCode::NOT_FOUND, None),
    ] {
        let (status, body) = lite
            .call("POST", &format!("{}/restore", d(doc)), None, who)
            .await;
        assert_eq!(status, expected, "{body}");
        if let Some(code) = code {
            assert_eq!(body["code"], code);
        }
    }
    let (status, _) = lite
        .call("POST", &format!("{}/restore", d(a)), None, &lite.owner)
        .await;
    assert_eq!(status, StatusCode::OK);
    let fresh = lite.fresh().await;
    assert_eq!(sqlite_stamps(&fresh, "documents", &[a]).await, vec![None]);
    assert!(sqlite_stamps(&fresh, "documents", &[child]).await[0].is_some());
    assert_eq!(journal(&fresh, lite.ws, "document.restored").await, (1, 1));
    fresh.close().await;
    let (status, _) = lite
        .call("POST", &format!("{}/restore", d(child)), None, &lite.owner)
        .await;
    assert_eq!(status, StatusCode::OK);

    // Past retention a trashed row is gone; an archived project refuses.
    let (status, _) = lite
        .call("POST", &format!("{}/trash", d(b)), None, &lite.owner)
        .await;
    assert_eq!(status, StatusCode::OK);
    sqlx::query("UPDATE documents SET deleted_at=deleted_at-31*86400000000 WHERE id=?1")
        .bind(blob(b))
        .execute(&lite.pool)
        .await
        .unwrap();
    let (status, _) = lite
        .call("POST", &format!("{}/restore", d(b)), None, &lite.owner)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = lite
        .call("POST", &format!("{}/trash", d(c)), None, &lite.owner)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = lite
        .call(
            "POST",
            &format!("/projects/{project}/archive"),
            None,
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    for path in [format!("{}/restore", d(c)), sort(a)] {
        let (status, _) = lite
            .call("POST", &path, Some(json!({"afterId": null})), &lite.owner)
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let fresh = lite.fresh().await;
    assert!(sqlite_stamps(&fresh, "documents", &[b, c])
        .await
        .iter()
        .all(Option::is_some));
    assert_eq!(journal(&fresh, lite.ws, "document.restored").await, (2, 2));
    fresh.close().await;
    lite.finish().await;
}
