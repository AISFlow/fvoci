use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::api::dto::{
    CreateProjectDocumentBody, DocumentMetaResponse, MoveDocumentBody, OkResponse,
    PatchDocumentBody, RequiredNullable, SortDocumentBody, TreeNodeResponse, TreeResponse,
};
use crate::auth::session::SessionUser;
use crate::db::documents::{CreateDocumentInput, UpdateDocumentMetaInput};
use crate::db::project_documents::{
    create_project_document_backend, get_project_document_backend,
    list_project_document_tree_backend, move_project_document, reorder_project_document,
    restore_project_document, trash_project_document, update_project_document_meta,
};
use crate::documents::export::ExportFormat;
use crate::error::{AppError, ProblemCode};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::routes::documents::{
    export_document, map_document_error, meta_response, parse_trash_children, DocumentApiError,
    TrashQuery,
};
use crate::http::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents",
            get(list_tree).post(create_document),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}",
            get(get_document)
                .patch(patch_document)
                .delete(trash_document),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/trash",
            post(trash_document),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/restore",
            post(restore_document),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/sort",
            post(sort_document),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/move",
            post(move_document),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/body",
            get(get_body),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/md",
            get(export_markdown),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/pdf",
            get(export_pdf),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/docx",
            get(export_docx),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/pptx",
            get(export_pptx),
        )
}

macro_rules! project_export_handler {
    ($name:ident, $format:expr) => {
        async fn $name(
            State(state): State<AppState>,
            headers: HeaderMap,
            jar: CookieJar,
            Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
        ) -> Result<Response, DocumentApiError> {
            export_document(
                &state,
                &headers,
                &jar,
                workspace_id,
                Some(project_id),
                document_id,
                $format,
            )
            .await
        }
    };
}

project_export_handler!(export_markdown, ExportFormat::Markdown);
project_export_handler!(export_pdf, ExportFormat::Pdf);
project_export_handler!(export_docx, ExportFormat::Docx);
project_export_handler!(export_pptx, ExportFormat::Pptx);

async fn list_tree(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
    query: Result<
        axum::extract::Query<crate::http::routes::documents::TreeQuery>,
        axum::extract::rejection::QueryRejection,
    >,
) -> Result<Json<TreeResponse>, DocumentApiError> {
    let axum::extract::Query(query) = query.map_err(AppError::from)?;
    let tag = crate::http::routes::documents::parse_tree_tag(query.tag.as_deref())?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsRead),
        Some(workspace_id),
    )
    .await?;
    let result = list_project_document_tree_backend(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        user_id,
        session_id,
        tag,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(nodes) => Ok(Json(TreeResponse {
            items: nodes
                .into_iter()
                .map(|node| TreeNodeResponse {
                    id: node.id.to_string(),
                    workspace_id: node.workspace_id.to_string(),
                    parent_id: node.parent_id.map(|id| id.to_string()),
                    project_id: node.project_id.map(|id| id.to_string()),
                    title: node.title,
                    icon: node.icon,
                    path: node.path,
                    sort_key: node.sort_key,
                    number: node.number,
                    status: node.status,
                })
                .collect(),
        })),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn create_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<CreateProjectDocumentBody>, JsonRejection>,
) -> Result<Response, DocumentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let title = body.title.trim();
    if !crate::db::documents::title_is_valid(title) {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    if let Some(Some(icon)) = body.icon.as_ref() {
        if !crate::db::documents::icon_is_valid(icon) {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
    }
    let parent_id = match body.parent_id {
        RequiredNullable::Missing => {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
        RequiredNullable::Null => None,
        RequiredNullable::Value(id) => Some(id),
    };
    if parent_id.is_none() {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = create_project_document_backend(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        user_id,
        session_id,
        CreateDocumentInput {
            parent_id,
            title,
            icon: body.icon.as_ref().map(|icon| icon.as_deref()),
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok((StatusCode::CREATED, Json(meta_response(&meta, true))).into_response()),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn get_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsRead),
        Some(workspace_id),
    )
    .await?;
    let result = get_project_document_backend(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        document_id,
        user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(&meta, true))),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn get_body(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
    Query(query): Query<crate::http::routes::document_body::BodyQuery>,
) -> Result<Json<crate::api::documents_dto::DocumentBodyResponse>, DocumentApiError> {
    crate::http::routes::document_body::read_body(
        &state,
        &headers,
        &jar,
        workspace_id,
        crate::db::document_ops::DocumentScope::Project(project_id),
        document_id,
        query.format.as_deref(),
    )
    .await
}

async fn patch_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Result<Json<PatchDocumentBody>, JsonRejection>,
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    if let Some(title) = body.title.as_ref() {
        if !crate::db::documents::title_is_valid(title) {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
    }
    if let Some(Some(icon)) = body.icon.as_ref() {
        if !crate::db::documents::icon_is_valid(icon) {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
    }
    if let Some(status) = body.status.as_ref() {
        if !crate::db::documents::status_is_valid(status) {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
    }
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = update_project_document_meta(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/project_documents.rs")
            .map_err(internal)?,
        workspace_id,
        project_id,
        document_id,
        user_id,
        session_id,
        UpdateDocumentMetaInput {
            title: body.title.as_deref().map(str::trim),
            icon: body.icon.as_ref().map(|value| value.as_deref()),
            status: body.status.as_deref(),
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(&meta, true))),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn move_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Result<Json<MoveDocumentBody>, JsonRejection>,
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = move_project_document(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/project_documents.rs")
            .map_err(internal)?,
        workspace_id,
        project_id,
        document_id,
        user_id,
        session_id,
        body.new_parent_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(&meta, true))),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn trash_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
    Query(query): Query<TrashQuery>,
) -> Result<Json<OkResponse>, DocumentApiError> {
    check_origin(&headers, &state.public_origin)?;
    let children = parse_trash_children(query.children.as_deref())?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = trash_project_document(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/project_documents.rs")
            .map_err(internal)?,
        workspace_id,
        project_id,
        document_id,
        user_id,
        session_id,
        children,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn restore_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<OkResponse>, DocumentApiError> {
    check_origin(&headers, &state.public_origin)?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = restore_project_document(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/project_documents.rs")
            .map_err(internal)?,
        workspace_id,
        project_id,
        document_id,
        user_id,
        session_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn sort_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Result<Json<SortDocumentBody>, JsonRejection>,
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = reorder_project_document(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/project_documents.rs")
            .map_err(internal)?,
        workspace_id,
        project_id,
        document_id,
        user_id,
        session_id,
        body.after_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(&meta, true))),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn require_session(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    access: crate::http::authz::Access,
    workspace_id: Option<Uuid>,
) -> Result<(SessionUser, Uuid, Uuid), DocumentApiError> {
    let auth =
        crate::http::authz::require_request_auth(state, headers, jar, access, workspace_id).await?;
    Ok((auth.user, auth.user_id, auth.credential_id))
}

fn internal(err: sqlx::Error) -> DocumentApiError {
    tracing::error!("database error: {}", err);
    AppError::internal().into()
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_create_http_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use crate::db::project_documents::selected_create_backend_tests::{counts, setup};
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn app(f: &Fixture) -> Router {
        router().with_state(AppState {
            realtime_mode:crate::config::RealtimeMode::Off,native_engine:None,
            auth:Arc::new(crate::auth::AuthService{db:crate::db::Db::from_backend(f.backend.clone()),password_keys:crate::auth::password::Keyring::parse(r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,"test").unwrap()}),
            branding_name:"FVOCI".into(),public_origin:"http://localhost".into(),cookie_secure:false,
            rate_limiter:crate::http::rate_limit::RateLimiter::new(),storage:crate::attachments::ObjectStorage::local(f.root.join("storage")),
            upload:crate::attachments::UploadLimits{part_size_bytes:24,max_file_size_bytes:1024,create_rate_per_5min:20,part_put_slots:crate::attachments::PartPutSlots::new(2)},
            collab:None,meili:None,search_embedder:None,markdown:None,import_wake:None,
            import_extractor_available:false,preview_extract:None,quota:Default::default(),
            mailer:Arc::new(crate::mail::Mailer::disabled()),streams:AppState::fresh_streams(),
        })
    }
    async fn session(f: &Fixture, credential: Uuid) -> String {
        let token = crate::auth::token::new_token();
        let expires = crate::db::identity::stored_now()
            + chrono::Duration::seconds(crate::auth::token::SESSION_TTL_SECS);
        sqlx::query("UPDATE sessions SET token_hash=?1,expires_at=?3 WHERE id=?2")
            .bind(token.hash)
            .bind(credential.as_bytes().as_slice())
            .bind(expires.timestamp_micros())
            .execute(&f.pool)
            .await
            .unwrap();
        token.token
    }
    async fn pat(f: &Fixture, scope: &str) -> String {
        let token = crate::auth::token::new_token();
        sqlx::query("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes) VALUES(?1,?2,?3,?4,'project create',?5)")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice()).bind(token.hash).bind(json!([scope]).to_string()).execute(&f.pool).await.unwrap();
        token.token
    }
    fn create_path(workspace: Uuid, project: Uuid) -> String {
        format!("/api/v1/workspaces/{workspace}/projects/{project}/documents")
    }
    async fn post(
        app: Router,
        path: &str,
        token: Option<&str>,
        bearer: bool,
        origin: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut request = axum::http::Request::builder()
            .method("POST")
            .uri(path)
            .extension(ConnectInfo(
                "127.0.0.1:31245".parse::<SocketAddr>().unwrap(),
            ))
            .header("origin", origin)
            .header("content-type", "application/json");
        if let Some(token) = token {
            request = if bearer {
                request.header("authorization", format!("Bearer {token}"))
            } else {
                request.header("cookie", format!("fvoci_session={token}"))
            };
        }
        let response = app
            .oneshot(
                request
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn sqlite_http_project_cookie_fixture_lookup_rejects_old_expiry_and_accepts_session_ttl()
    {
        let (f, credential, project) = setup().await;
        let before = counts(&f, project).await;
        let old =
            crate::db::identity::find_live_session_backend(&f.backend, &credential.to_string())
                .await;
        assert!(
            matches!(old, Err(sqlx::Error::Protocol(message)) if message == "SQLite instant out of range")
        );

        let before_issue = crate::db::identity::stored_now();
        let token = session(&f, credential).await;
        let after_issue = crate::db::identity::stored_now();
        let live = crate::db::identity::find_live_session_backend(
            &f.backend,
            &crate::auth::token::hash_token(&token),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(live.session_id, credential);
        assert_eq!(live.user_id, f.user);
        let ttl = chrono::Duration::seconds(crate::auth::token::SESSION_TTL_SECS);
        assert!(live.expires_at >= before_issue + ttl);
        assert!(live.expires_at <= after_issue + ttl);
        assert_eq!(counts(&f, project).await, before);
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_http_project_create_literal_request_metadata_number_order_and_publication() {
        let (f, credential, project) = setup().await;
        let token = session(&f, credential).await;
        let app = app(&f);
        let path = create_path(f.workspace, project);
        let before = counts(&f, project).await;
        let mut sort_keys = Vec::new();
        for (number, icon) in [(2, None), (3, Some("📄"))] {
            // The first request is the exact DTO shape that failed at OFF722.
            let mut request = json!({"parentId":f.document,"title":"OFF project body"});
            if let Some(icon) = icon {
                request["icon"] = json!(icon);
            }
            let (status, body) = post(
                app.clone(),
                &path,
                Some(&token),
                false,
                "http://localhost",
                request,
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            let id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
            assert!(!id.is_nil());
            assert_ne!(id, f.document);
            assert_eq!(body["workspaceId"], f.workspace.to_string());
            assert_eq!(body["projectId"], project.to_string());
            assert_eq!(body["parentId"], f.document.to_string());
            assert_eq!(body["title"], "OFF project body");
            assert_eq!(body["icon"], json!(icon));
            assert_eq!(body["status"], "draft");
            assert_eq!(
                body["schemaVersion"],
                crate::db::documents::DOCUMENT_SCHEMA_VERSION
            );
            assert_eq!(body["number"], number);
            assert_eq!(body["displayId"], format!("OFF-{number}"));
            assert_eq!(body["createdBy"], f.user.to_string());
            let row: (Vec<u8>,Vec<u8>,i64,String,String,String) = sqlx::query_as("SELECT parent_id,project_id,number,content_json,path,sort_key FROM documents WHERE workspace_id=?1 AND id=?2")
                .bind(f.workspace.as_bytes().as_slice()).bind(id.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
            assert_eq!(row.0, f.document.as_bytes());
            assert_eq!(row.1, project.as_bytes());
            assert_eq!(row.2, number);
            assert_eq!(
                serde_json::from_str::<Value>(&row.3).unwrap(),
                crate::db::documents::empty_document_json()
            );
            assert_eq!(row.4, format!("{}.{}", f.document.simple(), id.simple()));
            assert_eq!(body["path"], row.4);
            assert_eq!(body["sortKey"], row.5);
            sort_keys.push(row.5);
            for table in ["events", "audit_log"] {
                let rows: Vec<(Vec<u8>,String)> = sqlx::query_as(&format!("SELECT actor_user_id,payload FROM {table} WHERE workspace_id=?1 AND verb='document.created' AND target_id=?2"))
                    .bind(f.workspace.as_bytes().as_slice()).bind(id.as_bytes().as_slice()).fetch_all(&f.pool).await.unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].0, f.user.as_bytes());
                assert_eq!(
                    serde_json::from_str::<Value>(&rows[0].1).unwrap(),
                    json!({"documentId":id,"parentId":f.document,"title":"OFF project body","projectId":project})
                );
            }
        }
        assert!(sort_keys[0] < sort_keys[1]);
        assert_eq!(
            counts(&f, project).await,
            (before.0 + 2, 4, before.2 + 2, before.3 + 2)
        );
        drop(app);
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_http_project_create_input_origin_current_authority_and_pat_scope_denials() {
        let (f, credential, project) = setup().await;
        let token = session(&f, credential).await;
        let app = app(&f);
        let path = create_path(f.workspace, project);
        let valid = json!({"parentId":f.document,"title":"OFF project body"});
        let before = counts(&f, project).await;
        for invalid in [
            json!({"title":"missing parent"}),
            json!({"parentId":null,"title":"null parent"}),
            json!({"parentId":f.document,"title":" "}),
            json!({"parentId":f.document,"title":"valid","commandId":Uuid::now_v7()}),
        ] {
            assert_eq!(
                post(
                    app.clone(),
                    &path,
                    Some(&token),
                    false,
                    "http://localhost",
                    invalid
                )
                .await
                .0,
                StatusCode::BAD_REQUEST
            );
            assert_eq!(counts(&f, project).await, before);
        }
        let (status, body) = post(
            app.clone(),
            &path,
            Some(&token),
            false,
            "https://foreign.invalid",
            valid.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "origin_mismatch");
        assert_eq!(
            post(
                app.clone(),
                &path,
                None,
                false,
                "http://localhost",
                valid.clone()
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        let read_pat = pat(&f, "documents.read").await;
        assert_eq!(
            post(
                app.clone(),
                &path,
                Some(&read_pat),
                true,
                "http://localhost",
                valid.clone()
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        sqlx::query("UPDATE memberships SET role='guest' WHERE user_id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            post(
                app.clone(),
                &path,
                Some(&token),
                false,
                "http://localhost",
                valid.clone()
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        sqlx::query("UPDATE memberships SET role='owner' WHERE user_id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE documents SET project_id=NULL WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let (status, body) = post(
            app.clone(),
            &path,
            Some(&token),
            false,
            "http://localhost",
            valid.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "document_affiliation_mismatch");
        sqlx::query("UPDATE documents SET project_id=?1 WHERE id=?2")
            .bind(project.as_bytes().as_slice())
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let other_path = create_path(Uuid::now_v7(), project);
        assert_eq!(
            post(
                app.clone(),
                &other_path,
                Some(&token),
                false,
                "http://localhost",
                valid.clone()
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        sqlx::query("UPDATE sessions SET expires_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            post(
                app.clone(),
                &path,
                Some(&token),
                false,
                "http://localhost",
                valid.clone()
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(counts(&f, project).await, before);
        let write_pat = pat(&f, "documents.write").await;
        assert_eq!(
            post(
                app.clone(),
                &path,
                Some(&write_pat),
                true,
                "http://localhost",
                valid
            )
            .await
            .0,
            StatusCode::CREATED
        );
        drop(app);
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_http_project_create_real_fk_refusal_is_500_without_partial_effects() {
        let (f, credential, project) = setup().await;
        let token = session(&f, credential).await;
        let app = app(&f);
        let path = create_path(f.workspace, project);
        let valid = json!({"parentId":f.document,"title":"OFF project body"});
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        let before = counts(&f, project).await;
        sqlx::query("CREATE TABLE project_http_fk_probe(id BLOB REFERENCES documents(id)) STRICT")
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("CREATE TRIGGER project_http_refuse AFTER INSERT ON audit_log WHEN NEW.verb='document.created' BEGIN INSERT INTO project_http_fk_probe(id) VALUES(zeroblob(16)); END;").execute(&f.pool).await.unwrap();
        let (status, body) = post(
            app.clone(),
            &path,
            Some(&token),
            false,
            "http://localhost",
            valid.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["code"], "internal_error");
        assert_eq!(counts(&f, project).await, before);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM project_http_fk_probe")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            0
        );
        sqlx::query("DROP TRIGGER project_http_refuse")
            .execute(&f.pool)
            .await
            .unwrap();
        let (status, body) = post(
            app.clone(),
            &path,
            Some(&token),
            false,
            "http://localhost",
            valid,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["number"], 2);
        assert_eq!(
            counts(&f, project).await,
            (before.0 + 1, 3, before.2 + 1, before.3 + 1)
        );
        drop(app);
        f.close().await;
    }

    async fn get_metadata(
        app: Router,
        path: &str,
        token: Option<&str>,
        bearer: bool,
    ) -> (StatusCode, Value) {
        let mut request = axum::http::Request::builder().method("GET").uri(path);
        if let Some(token) = token {
            request = if bearer {
                request.header("authorization", format!("Bearer {token}"))
            } else {
                request.header("cookie", format!("fvoci_session={token}"))
            };
        }
        let response = app
            .oneshot(request.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn sqlite_http_project_metadata_cookie_pat_literal_and_current_denials() {
        let (f, credential, project) = setup().await;
        let token = session(&f, credential).await;
        let app = app(&f);
        let (status, created) = post(
            app.clone(),
            &create_path(f.workspace, project),
            Some(&token),
            false,
            "http://localhost",
            json!({"parentId":f.document,"title":"GET metadata 한글","icon":"📄"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
        let path = format!("{}/{}", create_path(f.workspace, project), id);
        let before = counts(&f, project).await;
        let read_token = pat(&f, "documents.read").await;
        let write_token = pat(&f, "documents.write").await;
        for (auth, bearer) in [(&token, false), (&read_token, true), (&write_token, true)] {
            let (status, body) = get_metadata(app.clone(), &path, Some(auth), bearer).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body, created);
            assert_eq!(body["id"], id.to_string());
            assert_eq!(body["workspaceId"], f.workspace.to_string());
            assert_eq!(body["projectId"], project.to_string());
            assert_eq!(body["parentId"], f.document.to_string());
            assert_eq!(body["title"], "GET metadata 한글");
            assert_eq!(body["icon"], "📄");
            assert_eq!(body["number"], 2);
            assert_eq!(body["displayId"], "OFF-2");
            assert_eq!(body["status"], "draft");
            assert_eq!(
                body["schemaVersion"],
                crate::db::documents::DOCUMENT_SCHEMA_VERSION
            );
            assert_eq!(body["version"], 1);
            assert_eq!(body["createdBy"], f.user.to_string());
            assert_eq!(
                body["path"],
                format!("{}.{}", f.document.simple(), id.simple())
            );
            assert!(!body["sortKey"].as_str().unwrap().is_empty());
            assert!(body.get("contentJson").is_none());
            assert!(body.get("tokenHash").is_none());
        }
        let wrong_scope_token = pat(&f, "tasks.read").await;
        let wrong_tenant_path = format!("{}/{}", create_path(Uuid::now_v7(), project), id);
        for (target, auth, bearer, expected) in [
            (path.as_str(), None, false, StatusCode::UNAUTHORIZED),
            (
                path.as_str(),
                Some(wrong_scope_token.as_str()),
                true,
                StatusCode::NOT_FOUND,
            ),
            (
                wrong_tenant_path.as_str(),
                Some(read_token.as_str()),
                true,
                StatusCode::NOT_FOUND,
            ),
            (
                wrong_tenant_path.as_str(),
                Some(token.as_str()),
                false,
                StatusCode::NOT_FOUND,
            ),
        ] {
            let (status, body) = get_metadata(app.clone(), target, auth, bearer).await;
            assert_eq!(status, expected, "{body}");
            assert!(body.get("title").is_none());
            assert!(body.get("displayId").is_none());
        }
        sqlx::query("UPDATE api_tokens SET expires_at=1 WHERE token_hash=?1")
            .bind(crate::auth::token::hash_token(&read_token))
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            get_metadata(app.clone(), &path, Some(&read_token), true)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let revoked_token = pat(&f, "documents.read").await;
        sqlx::query("DELETE FROM api_tokens WHERE token_hash=?1")
            .bind(crate::auth::token::hash_token(&revoked_token))
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            get_metadata(app.clone(), &path, Some(&revoked_token), true)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        for (deny, restore, target, expected) in [
            (
                "UPDATE sessions SET revoked_at=1 WHERE id=?1",
                "UPDATE sessions SET revoked_at=NULL WHERE id=?1",
                credential,
                StatusCode::UNAUTHORIZED,
            ),
            (
                "UPDATE projects SET visibility='private' WHERE id=?1",
                "UPDATE projects SET visibility='workspace' WHERE id=?1",
                project,
                StatusCode::NOT_FOUND,
            ),
            (
                "UPDATE documents SET deleted_at=1 WHERE id=?1",
                "UPDATE documents SET deleted_at=NULL WHERE id=?1",
                id,
                StatusCode::NOT_FOUND,
            ),
        ] {
            sqlx::query(deny)
                .bind(target.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            let (status, body) = get_metadata(app.clone(), &path, Some(&token), false).await;
            assert_eq!(status, expected, "{body}");
            assert!(body.get("title").is_none());
            assert!(body.get("displayId").is_none());
            sqlx::query(restore)
                .bind(target.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            let (status, body) = get_metadata(app.clone(), &path, Some(&token), false).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body, created);
        }
        sqlx::query("UPDATE sessions SET expires_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            get_metadata(app.clone(), &path, Some(&token), false)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(counts(&f, project).await, before);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&f.pool)
            .await
            .unwrap()
            .is_empty());
        drop(app);
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_http_project_metadata_driver_error_and_healthy_retry() {
        let (f, credential, project) = setup().await;
        let token = session(&f, credential).await;
        let app = app(&f);
        let path = format!("{}/{}", create_path(f.workspace, project), f.document);
        let before = counts(&f, project).await;
        sqlx::query("ALTER TABLE projects RENAME TO metadata_http_fault_projects")
            .execute(&f.pool)
            .await
            .unwrap();
        let (status, body) = get_metadata(app.clone(), &path, Some(&token), false).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        assert!(body.get("title").is_none());
        sqlx::query("ALTER TABLE metadata_http_fault_projects RENAME TO projects")
            .execute(&f.pool)
            .await
            .unwrap();
        let (status, body) = get_metadata(app.clone(), &path, Some(&token), false).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["id"], f.document.to_string());
        assert_eq!(body["title"], "S31");
        assert_eq!(body["displayId"], "OFF-1");
        assert_eq!(counts(&f, project).await, before);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&f.pool)
            .await
            .unwrap()
            .is_empty());
        drop(app);
        f.close().await;
    }
}
