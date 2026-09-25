#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use std::sync::Arc;

use axum::http::StatusCode;
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::AuthService;
use fvoci_server::db::{pool, Db};
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::state::AppState;
use fvoci_server::search::meili::{
    ensure_meili_index, search_source_id, upsert_meili_sources, MeiliConfig, SearchSource,
    SearchSourceKind,
};
use fvoci_server::search::text::index_document_text;
use project_harness::{
    add_workspace_user, admin_pool, create_project, insert_project_document,
    insert_stored_attachment, json_request, setup_session, TestDb,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

fn test_meili_config() -> MeiliConfig {
    let url = std::env::var("FVOCI_MEILI_URL").expect("FVOCI_MEILI_URL");
    let key = std::env::var("FVOCI_MEILI_KEY").expect("FVOCI_MEILI_KEY");
    let index_uid = format!("fvoci_{}", Uuid::now_v7().simple());
    MeiliConfig::new(url, key, index_uid)
}

async fn search_state(app_url: &str, meili: Option<MeiliConfig>) -> AppState {
    let pool = pool::connect_app(app_url).await.expect("app pool");
    let storage_root = std::env::temp_dir().join(format!("fvoci-search-test-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    AppState {
        auth: Arc::new(AuthService {
            db: Db::new(pool),
            password_keys: Keyring::parse(PEPPER, "test").expect("pepper"),
        }),
        branding_name: "FVOCI".to_string(),
        public_origin: "http://localhost".to_string(),
        cookie_secure: false,
        rate_limiter: RateLimiter::new(),
        storage: fvoci_server::attachments::LocalStorage::new(storage_root),
        upload: fvoci_server::attachments::UploadLimits {
            part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
            max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
            create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
        },
        collab: None,
        meili,
        mailer: std::sync::Arc::new(fvoci_server::mail::Mailer::disabled()),
    }
}

fn search_router(state: AppState) -> axum::Router {
    fvoci_server::http::router(state, None)
}

fn document_source(
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    document_id: Uuid,
    title: &str,
    body: &str,
) -> SearchSource {
    let text = index_document_text(title, body, "");
    SearchSource {
        id: search_source_id(SearchSourceKind::Document, &document_id.to_string(), None),
        kind: SearchSourceKind::Document,
        workspace_id: workspace_id.to_string(),
        project_id: project_id.map(|id| id.to_string()),
        document_id: Some(document_id.to_string()),
        task_id: None,
        comment_id: None,
        attachment_id: None,
        chunk_no: None,
        title: text.title,
        body: text.body,
        chosung: text.chosung,
        stem: text.stem,
        updated_at: 1,
    }
}

fn task_source(workspace_id: Uuid, project_id: Uuid, task_id: Uuid, title: &str) -> SearchSource {
    let text = index_document_text(title, "", "");
    SearchSource {
        id: search_source_id(SearchSourceKind::Task, &task_id.to_string(), None),
        kind: SearchSourceKind::Task,
        workspace_id: workspace_id.to_string(),
        project_id: Some(project_id.to_string()),
        document_id: None,
        task_id: Some(task_id.to_string()),
        comment_id: None,
        attachment_id: None,
        chunk_no: None,
        title: text.title,
        body: text.body,
        chosung: text.chosung,
        stem: text.stem,
        updated_at: 1,
    }
}

fn attachment_source(
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    document_id: Uuid,
    attachment_id: Uuid,
    name: &str,
    extract: &str,
) -> SearchSource {
    let text = index_document_text(name, extract, "");
    SearchSource {
        id: search_source_id(
            SearchSourceKind::Attachment,
            &attachment_id.to_string(),
            Some(0),
        ),
        kind: SearchSourceKind::Attachment,
        workspace_id: workspace_id.to_string(),
        project_id: project_id.map(|id| id.to_string()),
        document_id: Some(document_id.to_string()),
        task_id: None,
        comment_id: None,
        attachment_id: Some(attachment_id.to_string()),
        chunk_no: Some(0),
        title: text.title,
        body: text.body,
        chosung: text.chosung,
        stem: text.stem,
        updated_at: 1,
    }
}

async fn insert_wiki_document(
    admin: &PgPool,
    workspace_id: Uuid,
    document_id: Uuid,
    created_by: Uuid,
    number: i32,
    title: &str,
    body: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, parent_id, sort_key, project_id, number, status,
            schema_version, content_json, created_by, text
        ) VALUES (
            $1, $2, $3, $4, NULL, 'V', NULL, $5, 'published', 2,
            '{"type":"doc","content":[{"type":"paragraph"}]}'::jsonb, $6, $7
        )
        "#,
    )
    .bind(document_id)
    .bind(workspace_id)
    .bind(title)
    .bind(document_id.simple().to_string())
    .bind(number)
    .bind(created_by)
    .bind(body)
    .execute(admin)
    .await
    .expect("insert wiki document");
}

async fn set_document_copy(admin: &PgPool, document_id: Uuid, title: &str, body: &str) {
    sqlx::query("UPDATE fvoci.documents SET title = $2, text = $3 WHERE id = $1")
        .bind(document_id)
        .bind(title)
        .bind(body)
        .execute(admin)
        .await
        .expect("update document copy");
}

fn ids_of(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|item| item["id"].as_str().map(ToOwned::to_owned))
        .collect()
}

fn contains_id(body: &Value, id: Uuid) -> bool {
    ids_of(body).iter().any(|found| found == &id.to_string())
}

async fn search(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    q: &str,
    extra: &str,
) -> (StatusCode, Value) {
    let path = if extra.is_empty() {
        format!(
            "/api/v1/workspaces/{workspace_id}/search?q={}",
            urlencoding(q)
        )
    } else {
        format!(
            "/api/v1/workspaces/{workspace_id}/search?q={}&{extra}",
            urlencoding(q)
        )
    };
    json_request(app, "GET", &path, None, Some(cookie)).await
}

fn urlencoding(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// Seeds Meili with `upsert_meili_sources` (SearchSource via src/search/meili.rs).
#[tokio::test]
async fn search_workspace_query_contract_and_leak_matrix() {
    let harness = TestDb::bootstrap().await;
    let (_, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let meili = test_meili_config();
    ensure_meili_index(&meili)
        .await
        .unwrap_or_else(|e| panic!("ensure index: {e}"));
    let app = search_router(search_state(&harness.app_url, Some(meili.clone())).await);
    let admin = admin_pool(&harness).await;

    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "guest").await;
    let doomed = add_workspace_user(&admin, workspace_id, "member", "doomed").await;
    let priv_viewer = add_workspace_user(&admin, workspace_id, "member", "privview").await;

    let (status, other_ws) = json_request(
        app.clone(),
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name":"Other","slug":"otherws"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{other_ws:?}");
    let other_workspace_id = Uuid::parse_str(other_ws["id"].as_str().unwrap()).unwrap();

    let public_project =
        create_project(app.clone(), &owner_cookie, workspace_id, "PUB", "workspace").await;
    let private_project =
        create_project(app.clone(), &owner_cookie, workspace_id, "PRIV", "private").await;
    let public_id = Uuid::parse_str(public_project["id"].as_str().unwrap()).unwrap();
    let private_id = Uuid::parse_str(private_project["id"].as_str().unwrap()).unwrap();

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{public_id}/members"),
        Some(json!({"userId": guest.user_id.to_string(), "role": "viewer"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{private_id}/members"),
        Some(json!({"userId": priv_viewer.user_id.to_string(), "role": "viewer"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let token = format!("qvox{}", Uuid::now_v7().simple());
    let wiki_id = Uuid::now_v7();
    let moved_id = Uuid::now_v7();
    let pub_doc_id = Uuid::now_v7();
    let priv_doc_id = Uuid::now_v7();
    let trash_id = Uuid::now_v7();
    let arch_id = Uuid::now_v7();
    let stem_id = Uuid::now_v7();
    let chosung_id = Uuid::now_v7();
    let other_doc_id = Uuid::now_v7();

    insert_wiki_document(
        &admin,
        workspace_id,
        wiki_id,
        owner_id,
        101,
        &format!("{token} wiki"),
        "wiki body visible",
    )
    .await;
    insert_wiki_document(
        &admin,
        workspace_id,
        moved_id,
        owner_id,
        102,
        &format!("{token} moved"),
        "will move to private",
    )
    .await;
    insert_project_document(&admin, workspace_id, public_id, pub_doc_id, owner_id, 11).await;
    set_document_copy(
        &admin,
        pub_doc_id,
        &format!("{token} public"),
        "public body",
    )
    .await;
    insert_project_document(&admin, workspace_id, private_id, priv_doc_id, owner_id, 12).await;
    set_document_copy(
        &admin,
        priv_doc_id,
        &format!("{token} private"),
        "private body",
    )
    .await;
    insert_wiki_document(
        &admin,
        workspace_id,
        trash_id,
        owner_id,
        103,
        &format!("{token} trash"),
        "will be trashed",
    )
    .await;
    insert_wiki_document(
        &admin,
        workspace_id,
        arch_id,
        owner_id,
        104,
        &format!("{token} archived"),
        "will be archived",
    )
    .await;
    insert_wiki_document(
        &admin,
        workspace_id,
        stem_id,
        owner_id,
        105,
        "running shoes",
        "stem fixture body",
    )
    .await;
    insert_wiki_document(
        &admin,
        workspace_id,
        chosung_id,
        owner_id,
        106,
        "검색테스트",
        "chosung fixture body",
    )
    .await;
    insert_wiki_document(
        &admin,
        other_workspace_id,
        other_doc_id,
        owner_id,
        201,
        &format!("{token} otherws"),
        "cross workspace",
    )
    .await;

    let (status, live_task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{public_id}/tasks"),
        Some(json!({"title": format!("{token} task")})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{live_task:?}");
    let live_task_id = Uuid::parse_str(live_task["id"].as_str().unwrap()).unwrap();
    let (status, archived_task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{public_id}/tasks"),
        Some(json!({"title": format!("{token} archivedtask")})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{archived_task:?}");
    let archived_task_id = Uuid::parse_str(archived_task["id"].as_str().unwrap()).unwrap();
    sqlx::query("UPDATE fvoci.tasks SET archived_at = now() WHERE id = $1")
        .bind(archived_task_id)
        .execute(&admin)
        .await
        .unwrap();

    let attachment_id = insert_stored_attachment(&admin, workspace_id, priv_doc_id, owner_id).await;
    sqlx::query("UPDATE fvoci.attachments SET name = $2, extract_text = $3 WHERE id = $1")
        .bind(attachment_id)
        .bind(format!("{token} attach"))
        .bind("private attachment extract")
        .execute(&admin)
        .await
        .unwrap();

    // Indexer paused: trash/archive/move happen in PG only after Meili seed.
    upsert_meili_sources(
        &meili,
        &[
            document_source(
                workspace_id,
                None,
                wiki_id,
                &format!("{token} wiki"),
                "wiki body visible",
            ),
            document_source(
                workspace_id,
                None,
                moved_id,
                &format!("{token} moved"),
                "will move to private",
            ),
            document_source(
                workspace_id,
                Some(public_id),
                pub_doc_id,
                &format!("{token} public"),
                "public body",
            ),
            document_source(
                workspace_id,
                Some(private_id),
                priv_doc_id,
                &format!("{token} private"),
                "private body",
            ),
            document_source(
                workspace_id,
                None,
                trash_id,
                &format!("{token} trash"),
                "will be trashed",
            ),
            document_source(
                workspace_id,
                None,
                arch_id,
                &format!("{token} archived"),
                "will be archived",
            ),
            document_source(
                workspace_id,
                None,
                stem_id,
                "running shoes",
                "stem fixture body",
            ),
            document_source(
                workspace_id,
                None,
                chosung_id,
                "검색테스트",
                "chosung fixture body",
            ),
            document_source(
                other_workspace_id,
                None,
                other_doc_id,
                &format!("{token} otherws"),
                "cross workspace",
            ),
            task_source(
                workspace_id,
                public_id,
                live_task_id,
                &format!("{token} task"),
            ),
            task_source(
                workspace_id,
                public_id,
                archived_task_id,
                &format!("{token} archivedtask"),
            ),
            attachment_source(
                workspace_id,
                Some(private_id),
                priv_doc_id,
                attachment_id,
                &format!("{token} attach"),
                "private attachment extract",
            ),
        ],
    )
    .await
    .unwrap_or_else(|e| panic!("upsert_meili_sources: {e}"));

    sqlx::query("UPDATE fvoci.documents SET deleted_at = now() WHERE id = $1")
        .bind(trash_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.documents SET status = 'archived' WHERE id = $1")
        .bind(arch_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.documents SET project_id = $2, number = 13 WHERE id = $1")
        .bind(moved_id)
        .bind(private_id)
        .execute(&admin)
        .await
        .unwrap();

    let (status, owner_hits) = search(app.clone(), &owner_cookie, workspace_id, &token, "").await;
    assert_eq!(status, StatusCode::OK, "{owner_hits:?}");
    assert!(contains_id(&owner_hits, wiki_id), "{owner_hits:?}");
    assert!(contains_id(&owner_hits, pub_doc_id), "{owner_hits:?}");
    assert!(contains_id(&owner_hits, priv_doc_id), "{owner_hits:?}");
    assert!(contains_id(&owner_hits, live_task_id), "{owner_hits:?}");
    assert!(
        !contains_id(&owner_hits, trash_id),
        "trashed still visible: {owner_hits:?}"
    );
    assert!(
        !contains_id(&owner_hits, arch_id),
        "archived still visible: {owner_hits:?}"
    );
    assert!(
        !contains_id(&owner_hits, archived_task_id),
        "archived task still visible: {owner_hits:?}"
    );
    assert!(
        !contains_id(&owner_hits, other_doc_id),
        "cross-workspace leak: {owner_hits:?}"
    );

    let (status, member_hits) = search(app.clone(), &member.cookie, workspace_id, &token, "").await;
    assert_eq!(status, StatusCode::OK, "{member_hits:?}");
    assert!(contains_id(&member_hits, wiki_id), "{member_hits:?}");
    assert!(contains_id(&member_hits, pub_doc_id), "{member_hits:?}");
    assert!(
        !contains_id(&member_hits, priv_doc_id),
        "private project leaked to member: {member_hits:?}"
    );
    assert!(
        !contains_id(&member_hits, moved_id),
        "wiki->private stale index leaked: {member_hits:?}"
    );
    assert!(
        !contains_id(&member_hits, attachment_id),
        "private attachment leaked: {member_hits:?}"
    );

    let (status, guest_hits) = search(app.clone(), &guest.cookie, workspace_id, &token, "").await;
    assert_eq!(status, StatusCode::OK, "{guest_hits:?}");
    assert!(
        !contains_id(&guest_hits, wiki_id),
        "guest saw wiki without grant: {guest_hits:?}"
    );
    assert!(contains_id(&guest_hits, pub_doc_id), "{guest_hits:?}");
    assert!(!contains_id(&guest_hits, priv_doc_id), "{guest_hits:?}");

    let (status, viewer_hits) =
        search(app.clone(), &priv_viewer.cookie, workspace_id, &token, "").await;
    assert_eq!(status, StatusCode::OK, "{viewer_hits:?}");
    assert!(contains_id(&viewer_hits, priv_doc_id), "{viewer_hits:?}");

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{private_id}/members/{}",
            priv_viewer.user_id
        ),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, after_remove) =
        search(app.clone(), &priv_viewer.cookie, workspace_id, &token, "").await;
    assert_eq!(status, StatusCode::OK, "{after_remove:?}");
    assert!(
        !contains_id(&after_remove, priv_doc_id),
        "removed project member still sees private: {after_remove:?}"
    );

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/members/{}",
            doomed.user_id
        ),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, doomed_hits) = search(app.clone(), &doomed.cookie, workspace_id, &token, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{doomed_hits:?}");

    let (status, stem_hits) = search(app.clone(), &owner_cookie, workspace_id, "runs", "").await;
    assert_eq!(status, StatusCode::OK, "{stem_hits:?}");
    assert!(contains_id(&stem_hits, stem_id), "stem miss: {stem_hits:?}");

    let (status, chosung_hits) = search(app.clone(), &owner_cookie, workspace_id, "ㄱㅅ", "").await;
    assert_eq!(status, StatusCode::OK, "{chosung_hits:?}");
    assert!(
        contains_id(&chosung_hits, chosung_id),
        "chosung miss: {chosung_hits:?}"
    );

    let (status, injected) = search(
        app.clone(),
        &owner_cookie,
        workspace_id,
        &token,
        "projectId=not-a-uuid",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{injected:?}");

    let (status, injected_type) = search(
        app.clone(),
        &owner_cookie,
        workspace_id,
        &token,
        "type=document%22%20OR%20kind%3D%22task",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{injected_type:?}");

    let (status, injected_q) = search(
        app.clone(),
        &owner_cookie,
        workspace_id,
        &format!("{token}\" OR kind = \"document"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{injected_q:?}");
    assert!(!contains_id(&injected_q, other_doc_id), "{injected_q:?}");

    let (status, comments) = search(
        app.clone(),
        &owner_cookie,
        workspace_id,
        &token,
        "type=comment",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{comments:?}");
    assert_eq!(comments["items"].as_array().map(Vec::len).unwrap_or(99), 0);

    let (status, empty) = search(app.clone(), &owner_cookie, workspace_id, "   ", "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{empty:?}");
    let (status, cursor) = search(
        app.clone(),
        &owner_cookie,
        workspace_id,
        &token,
        "cursor=%%%",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{cursor:?}");
    assert_eq!(cursor["params"]["code"], "invalid_cursor");

    let down = search_router(
        search_state(
            &harness.app_url,
            Some(MeiliConfig::new(
                "http://127.0.0.1:1".into(),
                "x".into(),
                "fvoci_down".into(),
            )),
        )
        .await,
    );
    let (status, problem) = search(down, &owner_cookie, workspace_id, &token, "").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{problem:?}");
    assert_eq!(problem["code"], "search_unavailable");

    let (status, recovered) = search(
        app,
        &owner_cookie,
        workspace_id,
        &format!("{token} wiki"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{recovered:?}");
    assert!(contains_id(&recovered, wiki_id), "{recovered:?}");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn search_guest_wiki_group_grant_is_hydrated() {
    let harness = TestDb::bootstrap().await;
    let (_, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let meili = test_meili_config();
    ensure_meili_index(&meili)
        .await
        .unwrap_or_else(|e| panic!("ensure index: {e}"));
    let app = search_router(search_state(&harness.app_url, Some(meili.clone())).await);
    let admin = admin_pool(&harness).await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "wiki-grant").await;

    let token = format!("gwiki{}", Uuid::now_v7().simple());
    let wiki_id = Uuid::now_v7();
    insert_wiki_document(
        &admin,
        workspace_id,
        wiki_id,
        owner_id,
        301,
        &format!("{token} granted"),
        "guest wiki body",
    )
    .await;
    upsert_meili_sources(
        &meili,
        &[document_source(
            workspace_id,
            None,
            wiki_id,
            &format!("{token} granted"),
            "guest wiki body",
        )],
    )
    .await
    .unwrap_or_else(|e| panic!("upsert_meili_sources: {e}"));

    let (status, before) = search(app.clone(), &guest.cookie, workspace_id, &token, "").await;
    assert_eq!(status, StatusCode::OK, "{before:?}");
    assert!(
        !contains_id(&before, wiki_id),
        "guest saw wiki without grant: {before:?}"
    );

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "검색뷰어"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let group_id = created["id"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": guest.user_id.to_string()})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        Some(json!({"groupId": group_id, "role": "viewer"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, after) = search(app, &guest.cookie, workspace_id, &token, "").await;
    assert_eq!(status, StatusCode::OK, "{after:?}");
    assert!(
        contains_id(&after, wiki_id),
        "guest wiki group grant missing from search: {after:?}"
    );

    admin.close().await;
    harness.cleanup().await;
}
