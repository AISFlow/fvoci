#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use project_harness::{
    add_workspace_user, admin_pool, app_pool, create_project, insert_minimal_project, json_request,
    setup_session, test_peer, TestDb,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: &str,
) -> (StatusCode, Value) {
    json_request(app.clone(), method, path, body, Some(cookie)).await
}

async fn call_bearer(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    secret: &str,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", "http://localhost")
        .header("authorization", format!("Bearer {secret}"));
    let request = if let Some(body) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let mut request = request;
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let json = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or(json!({}))
    };
    (status, json)
}

async fn create_pat(
    app: &axum::Router,
    cookie: &str,
    ws: Uuid,
    name: &str,
    scopes: &[&str],
) -> (String, String) {
    let (status, body) = call(
        app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/api-tokens"),
        Some(json!({ "name": name, "scopes": scopes })),
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (
        body["id"].as_str().expect("token id").to_string(),
        body["token"].as_str().expect("token secret").to_string(),
    )
}

fn listed_kinds(items: &Value) -> Vec<&str> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["kind"].as_str().unwrap())
        .collect()
}

async fn workspace_counts(admin: &PgPool, ws: Uuid) -> (i64, i64) {
    let documents: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.documents WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(admin)
            .await
            .unwrap();
    let tasks: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.tasks WHERE workspace_id = $1")
        .bind(ws)
        .fetch_one(admin)
        .await
        .unwrap();
    (documents, tasks)
}

async fn add_project_member(
    app: &axum::Router,
    cookie: &str,
    ws: Uuid,
    project_id: &str,
    user_id: Uuid,
    role: &str,
) {
    let (status, body) = call(
        app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/members"),
        Some(json!({ "userId": user_id, "role": role })),
        cookie,
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
}

#[tokio::test]
async fn templates_list_create_apply_and_guest_denied() {
    let db = TestDb::bootstrap().await;
    let (app, owner_cookie, _owner, ws) = setup_session(&db).await;
    let project = create_project(app.clone(), &owner_cookie, ws, "PRJ", "workspace").await;
    let project_id = project["id"].as_str().unwrap();

    let base = format!("/api/v1/workspaces/{ws}/templates");
    let (status, listed) = call(&app, "GET", &base, None, &owner_cookie).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["items"].as_array().unwrap().len(), 0);

    let (status, doc_tpl) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "document",
            "title": "Doc tpl",
            "payload": { "title": "Doc tpl" }
        })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{doc_tpl}");
    let doc_tpl_id = doc_tpl["id"].as_str().unwrap();

    let (status, applied) = call(
        &app,
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({})),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{applied}");
    assert_eq!(applied["kind"], "document");
    assert!(applied["displayId"].as_str().unwrap().starts_with("WIKI-"));

    let (status, task_tpl) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "task",
            "title": "Task tpl",
            "payload": { "title": "Task tpl" }
        })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task_tpl}");
    let task_tpl_id = task_tpl["id"].as_str().unwrap();

    let (status, applied_task) = call(
        &app,
        "POST",
        &format!("{base}/{task_tpl_id}/apply"),
        Some(json!({ "projectId": project_id })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{applied_task}");
    assert_eq!(applied_task["kind"], "task");
    assert!(applied_task["displayId"]
        .as_str()
        .unwrap()
        .starts_with("PRJ-"));

    let admin = admin_pool(&db).await;
    let guest = add_workspace_user(&admin, ws, "guest", "guest").await;
    admin.close().await;
    let (status, _) = call(&app, "GET", &base, None, &guest.cookie).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    db.cleanup().await;
}

#[tokio::test]
async fn templates_rls_denies_cross_tenant_reads() {
    let db = TestDb::bootstrap().await;
    let (app, cookie, _user, ws) = setup_session(&db).await;
    let base = format!("/api/v1/workspaces/{ws}/templates");
    let (status, created) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "document",
            "title": "Secret",
            "payload": { "title": "Secret" }
        })),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let tpl_id = created["id"].as_str().unwrap();

    let app_pool = app_pool(&db).await;
    let tpl_uuid = Uuid::parse_str(tpl_id).unwrap();
    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(ws.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.templates WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(count, 1);
    tx.commit().await.unwrap();

    let other = Uuid::now_v7();
    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(other.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let denied: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.templates WHERE id = $1")
        .bind(tpl_uuid)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(denied, 0);
    tx.rollback().await.unwrap();
    app_pool.close().await;

    db.cleanup().await;
}

#[tokio::test]
async fn templates_pat_kind_scopes_and_revoked_token() {
    let db = TestDb::bootstrap().await;
    let (app, owner_cookie, _owner, ws) = setup_session(&db).await;
    let project = create_project(app.clone(), &owner_cookie, ws, "PAT", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let base = format!("/api/v1/workspaces/{ws}/templates");

    let (status, doc_tpl) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "document",
            "title": "Doc",
            "payload": { "title": "Doc" }
        })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{doc_tpl}");
    let doc_tpl_id = doc_tpl["id"].as_str().unwrap();

    let (status, task_tpl) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "task",
            "title": "Task",
            "payload": { "title": "Task" }
        })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task_tpl}");
    let task_tpl_id = task_tpl["id"].as_str().unwrap();

    let (_id, doc_read) =
        create_pat(&app, &owner_cookie, ws, "doc-read", &["documents.read"]).await;
    let (status, listed) = call_bearer(app.clone(), "GET", &base, None, &doc_read).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed_kinds(&listed["items"]), ["document"]);
    let (status, denied) = call_bearer(
        app.clone(),
        "POST",
        &base,
        Some(json!({
            "kind": "document",
            "title": "Nope",
            "payload": { "title": "Nope" }
        })),
        &doc_read,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");
    let (status, denied) = call_bearer(
        app.clone(),
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({})),
        &doc_read,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");

    let (_id, task_read) = create_pat(&app, &owner_cookie, ws, "task-read", &["tasks.read"]).await;
    let (status, listed) = call_bearer(app.clone(), "GET", &base, None, &task_read).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed_kinds(&listed["items"]), ["task"]);

    let (_id, doc_write) =
        create_pat(&app, &owner_cookie, ws, "doc-write", &["documents.write"]).await;
    let (status, listed) = call_bearer(app.clone(), "GET", &base, None, &doc_write).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed_kinds(&listed["items"]), ["document"]);
    let (status, created) = call_bearer(
        app.clone(),
        "POST",
        &base,
        Some(json!({
            "kind": "document",
            "title": "Pat doc",
            "payload": { "title": "Pat doc" }
        })),
        &doc_write,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (status, denied) = call_bearer(
        app.clone(),
        "POST",
        &base,
        Some(json!({
            "kind": "task",
            "title": "Pat task",
            "payload": { "title": "Pat task" }
        })),
        &doc_write,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");
    let (status, applied) = call_bearer(
        app.clone(),
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({})),
        &doc_write,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{applied}");
    assert_eq!(applied["kind"], "document");
    let (status, denied) = call_bearer(
        app.clone(),
        "POST",
        &format!("{base}/{task_tpl_id}/apply"),
        Some(json!({ "projectId": project_id })),
        &doc_write,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");

    let (_id, task_write) =
        create_pat(&app, &owner_cookie, ws, "task-write", &["tasks.write"]).await;
    let (status, listed) = call_bearer(app.clone(), "GET", &base, None, &task_write).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed_kinds(&listed["items"]), ["task"]);
    let (status, applied) = call_bearer(
        app.clone(),
        "POST",
        &format!("{base}/{task_tpl_id}/apply"),
        Some(json!({ "projectId": project_id })),
        &task_write,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{applied}");
    assert_eq!(applied["kind"], "task");
    let (status, denied) = call_bearer(
        app.clone(),
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({})),
        &task_write,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");

    let (token_id, revoked_secret) = create_pat(
        &app,
        &owner_cookie,
        ws,
        "revoked",
        &["documents.write", "tasks.write"],
    )
    .await;
    let (status, listed) = call_bearer(app.clone(), "GET", &base, None, &revoked_secret).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let (status, revoked, ..) = project_harness::json_request_with_headers(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{ws}/api-tokens/{token_id}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    let (status, denied) = call_bearer(app.clone(), "GET", &base, None, &revoked_secret).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{denied}");
    assert_eq!(denied["code"], "authentication_required");
    let (status, denied) = call_bearer(
        app.clone(),
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({})),
        &revoked_secret,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{denied}");

    db.cleanup().await;
}

#[tokio::test]
async fn templates_guest_apply_archive_and_foreign_targets() {
    let db = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, ws) = setup_session(&db).await;
    let allowed = create_project(app.clone(), &owner_cookie, ws, "OKP", "workspace").await;
    let denied = create_project(app.clone(), &owner_cookie, ws, "NOPE", "private").await;
    let allowed_id = allowed["id"].as_str().unwrap().to_string();
    let denied_id = denied["id"].as_str().unwrap().to_string();
    let base = format!("/api/v1/workspaces/{ws}/templates");

    let (status, doc_tpl) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "document",
            "title": "Guest doc",
            "payload": { "title": "Guest doc" }
        })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{doc_tpl}");
    let doc_tpl_id = doc_tpl["id"].as_str().unwrap().to_string();

    let (status, task_tpl) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "task",
            "title": "Guest task",
            "payload": { "title": "Guest task" }
        })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task_tpl}");
    let task_tpl_id = task_tpl["id"].as_str().unwrap().to_string();

    let admin = admin_pool(&db).await;
    let guest = add_workspace_user(&admin, ws, "guest", "guest").await;
    add_project_member(
        &app,
        &owner_cookie,
        ws,
        &allowed_id,
        guest.user_id,
        "member",
    )
    .await;

    let (status, _) = call(&app, "GET", &base, None, &guest.cookie).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, created) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "document",
            "title": "Guest create",
            "payload": { "title": "Guest create" }
        })),
        &guest.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{created}");

    let (status, denied_wiki) = call(
        &app,
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({})),
        &guest.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied_wiki}");
    let (status, denied_other) = call(
        &app,
        "POST",
        &format!("{base}/{task_tpl_id}/apply"),
        Some(json!({ "projectId": denied_id })),
        &guest.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied_other}");

    let (status, applied_doc) = call(
        &app,
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({ "projectId": allowed_id })),
        &guest.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{applied_doc}");
    assert_eq!(applied_doc["kind"], "document");
    assert!(applied_doc["displayId"]
        .as_str()
        .unwrap()
        .starts_with("OKP-"));

    let (status, applied_task) = call(
        &app,
        "POST",
        &format!("{base}/{task_tpl_id}/apply"),
        Some(json!({ "projectId": allowed_id })),
        &guest.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{applied_task}");
    assert_eq!(applied_task["kind"], "task");
    assert!(applied_task["displayId"]
        .as_str()
        .unwrap()
        .starts_with("OKP-"));

    let (docs_before, tasks_before) = workspace_counts(&admin, ws).await;

    let (status, wiki_doc) = call(
        &app,
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({})),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{wiki_doc}");
    let wiki_parent = wiki_doc["id"].as_str().unwrap();

    let (status, foreign_parent) = call(
        &app,
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({
            "projectId": allowed_id,
            "parentId": wiki_parent
        })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{foreign_parent}");

    let other_ws = Uuid::now_v7();
    let other_project = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, 'Other')")
        .bind(other_ws)
        .bind(format!("other-{}", &other_ws.simple().to_string()[20..]))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(other_ws)
    .bind(owner_id)
    .execute(&admin)
    .await
    .unwrap();
    insert_minimal_project(
        &admin,
        other_ws,
        other_project,
        "FRN",
        owner_id,
        "workspace",
    )
    .await;

    let (status, foreign_project) = call(
        &app,
        "POST",
        &format!("{base}/{task_tpl_id}/apply"),
        Some(json!({ "projectId": other_project })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{foreign_project}");

    let (status, archived) = call(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{denied_id}/archive"),
        Some(json!({})),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{archived}");
    let (status, archived_apply) = call(
        &app,
        "POST",
        &format!("{base}/{task_tpl_id}/apply"),
        Some(json!({ "projectId": denied_id })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{archived_apply}");
    assert_eq!(archived_apply["code"], "project_archived");

    let (docs_after, tasks_after) = workspace_counts(&admin, ws).await;
    assert_eq!(
        docs_after,
        docs_before + 1,
        "only the wiki apply created a document"
    );
    assert_eq!(
        tasks_after, tasks_before,
        "failed applies must not create tasks"
    );

    admin.close().await;
    db.cleanup().await;
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

    pub(super) async fn scalar(pool: &SqlitePool, sql: &'static str, ids: &[Uuid]) -> i64 {
        let mut query = sqlx::query_scalar::<_, i64>(sql);
        for id in ids {
            query = query.bind(blob(*id));
        }
        query.fetch_one(pool).await.unwrap()
    }

    pub(super) async fn text(pool: &SqlitePool, sql: &'static str, ids: &[Uuid]) -> Option<String> {
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

/// Templates on SQLite: create and list by the workspace base permission and
/// token kinds, then apply as a project document, a wiki document and a task,
/// each read back on a new connection; refusals create nothing.
#[tokio::test]
async fn sqlite_templates_keep_the_postgres_contract() {
    use sqlite_port::{id_of, scalar, text, Lite};

    let lite = Lite::new().await;
    let (project, root) = lite.project("TPL", "workspace").await;
    let (archived, _) = lite.project("ARC", "workspace").await;
    let (status, _) = lite
        .call(
            "POST",
            &format!("/projects/{archived}/archive"),
            None,
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let member = lite.member(lite.ws, "member", "member").await;
    let guest = lite.member(lite.ws, "guest", "guest").await;
    let stranger_ws = lite.workspace("strangers").await;
    let stranger = lite.member(stranger_ws, "owner", "stranger").await;

    let (status, doc) = lite
        .call(
            "POST",
            "/templates",
            Some(json!({"kind": "document", "title": " Doc T ", "payload": {"title": "From payload", "body": [1, {"x": "y"}]}})),
            &member,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{doc}");
    assert_eq!(doc["title"], "Doc T");
    assert_eq!(doc["createdBy"], member.id.to_string());
    let (status, task) = lite
        .call(
            "POST",
            "/templates",
            Some(json!({"kind": "task", "title": "Task T", "payload": {}})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let (doc_id, task_id) = (id_of(&doc["id"]), id_of(&task["id"]));

    for (who, body, expected) in [
        (
            &lite.owner,
            json!({"kind": "task", "title": "Bad\u{0}", "payload": {}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            &lite.owner,
            json!({"kind": "task", "title": "T", "payload": {"note": "a\u{0}b"}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            &lite.owner,
            json!({"kind": "task", "title": "T", "payload": {"k\u{0}": 1}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            &lite.owner,
            json!({"kind": "task", "title": "T", "payload": [1]}),
            StatusCode::BAD_REQUEST,
        ),
        (
            &lite.owner,
            json!({"kind": "task", "title": "T", "payload": {"title": 5}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            &lite.owner,
            json!({"kind": "task", "title": "   ", "payload": {}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            &guest,
            json!({"kind": "task", "title": "T", "payload": {}}),
            StatusCode::NOT_FOUND,
        ),
        (
            &stranger,
            json!({"kind": "task", "title": "T", "payload": {}}),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (status, refused) = lite
            .call("POST", "/templates", Some(body.clone()), who)
            .await;
        assert_eq!(status, expected, "{body}: {refused}");
    }
    let fresh = lite.fresh().await;
    assert_eq!(
        scalar(&fresh, "SELECT count(*) FROM templates", &[]).await,
        2
    );
    assert_eq!(
        text(
            &fresh,
            "SELECT kind||'|'||title||'|'||json_extract(payload,'$.body[1].x') FROM templates WHERE id=?1",
            &[doc_id]
        )
        .await
        .as_deref(),
        Some("document|Doc T|y")
    );
    fresh.close().await;

    // Newest first; a token lists only the kinds it can read.
    let (status, list) = lite.call("GET", "/templates", None, &member).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let ids: Vec<String> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, [task_id.to_string(), doc_id.to_string()]);
    let (status, _) = lite.call("GET", "/templates", None, &guest).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let reader = lite.token(&lite.owner, r#"["documents.read"]"#).await;
    let (status, list) = lite.bearer("GET", "/templates", None, &reader).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["id"], doc_id.to_string());

    let apply = |template: Uuid| format!("/templates/{template}/apply");
    let (status, placed) = lite
        .call(
            "POST",
            &apply(doc_id),
            Some(json!({"projectId": project})),
            &member,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{placed}");
    assert_eq!(placed["kind"], "document");
    assert_eq!(placed["displayId"], "TPL-2");
    let (status, wiki) = lite
        .call("POST", &apply(doc_id), Some(json!({})), &member)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{wiki}");
    assert!(
        wiki["displayId"].as_str().unwrap().starts_with("WIKI-"),
        "{wiki}"
    );
    let writer = lite.token(&lite.owner, r#"["tasks.write"]"#).await;
    let (status, applied) = lite
        .bearer(
            "POST",
            &apply(task_id),
            Some(json!({"projectId": project})),
            &writer,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{applied}");
    assert_eq!(applied["kind"], "task");
    assert!(applied["displayId"].as_str().unwrap().starts_with("TPL-"));
    let fresh = lite.fresh().await;
    assert_eq!(
        scalar(
            &fresh,
            "SELECT count(*) FROM documents WHERE id=?1 AND project_id=?2 AND parent_id=?3 AND title='From payload'",
            &[id_of(&placed["id"]), project, root]
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            &fresh,
            "SELECT count(*) FROM documents WHERE id=?1 AND project_id IS NULL AND title='From payload'",
            &[id_of(&wiki["id"])]
        )
        .await,
        1
    );
    let task_row = id_of(&applied["id"]);
    assert_eq!(
        scalar(
            &fresh,
            "SELECT count(*) FROM tasks t JOIN collection_items i ON i.task_id=t.id JOIN collections c ON c.id=i.collection_id
             WHERE t.id=?1 AND t.project_id=?2 AND t.title='Task T' AND c.project_id=?2 AND c.kind='task'",
            &[task_row, project]
        )
        .await,
        1
    );
    fresh.close().await;

    // Refusals create nothing.
    let documents_before = {
        let fresh = lite.fresh().await;
        let counts = (
            scalar(&fresh, "SELECT count(*) FROM documents", &[]).await,
            scalar(&fresh, "SELECT count(*) FROM tasks", &[]).await,
        );
        fresh.close().await;
        counts
    };
    for (template, body, who, expected, code) in [
        (
            task_id,
            json!({}),
            &lite.owner,
            StatusCode::BAD_REQUEST,
            None,
        ),
        (
            task_id,
            json!({"projectId": archived}),
            &lite.owner,
            StatusCode::CONFLICT,
            Some("project_archived"),
        ),
        (
            doc_id,
            json!({"projectId": project}),
            &guest,
            StatusCode::NOT_FOUND,
            None,
        ),
        (doc_id, json!({}), &guest, StatusCode::NOT_FOUND, None),
        (doc_id, json!({}), &stranger, StatusCode::NOT_FOUND, None),
        (
            Uuid::now_v7(),
            json!({"projectId": project}),
            &lite.owner,
            StatusCode::NOT_FOUND,
            None,
        ),
        (
            doc_id,
            json!({"projectId": Uuid::now_v7()}),
            &lite.owner,
            StatusCode::NOT_FOUND,
            None,
        ),
    ] {
        let (status, body) = lite.call("POST", &apply(template), Some(body), who).await;
        assert_eq!(status, expected, "{body}");
        if let Some(code) = code {
            assert_eq!(body["code"], code);
        }
    }
    let (status, _) = lite
        .bearer(
            "POST",
            &apply(doc_id),
            Some(json!({"projectId": project})),
            &writer,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a tasks.write token cannot apply documents"
    );
    let fresh = lite.fresh().await;
    assert_eq!(
        (
            scalar(&fresh, "SELECT count(*) FROM documents", &[]).await,
            scalar(&fresh, "SELECT count(*) FROM tasks", &[]).await,
        ),
        documents_before
    );
    fresh.close().await;
    lite.finish().await;
}
