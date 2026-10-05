use std::net::SocketAddr;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::api::dto::{
    CommentListResponse, CommentOutput, CommentReactionBody, CommentReactionSummary,
    CreateCommentBody, OkResponse, PatchCommentBody,
};
use crate::auth::scopes::{grants_api_token_scope, ApiTokenScope};
use crate::db::comments::{
    comment_output, comment_write_kind, create_document_comment, create_project_document_comment,
    create_task_comment, list_document_comments_backend as list_document_comments,
    list_project_document_comments, list_task_comments, purge_comment, resolve_comment,
    set_comment_reaction, unresolve_comment, update_comment, CommentDbError, CommentListQuery,
    CommentWriteKind, CreateCommentInput, PatchCommentInput, ReactionInput,
};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{self, Access, RequestAuth};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentListQueryParams {
    pub limit: Option<i32>,
    pub cursor: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments",
            get(list_document_comments_route).post(create_document_comment_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/comments",
            get(list_project_document_comments_route).post(create_project_document_comment_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments",
            get(list_task_comments_route).post(create_task_comment_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/comments/{comment_id}",
            axum::routing::patch(patch_comment_route).delete(delete_comment_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/comments/{comment_id}/resolve",
            post(resolve_comment_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/comments/{comment_id}/unresolve",
            post(unresolve_comment_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/comments/{comment_id}/reactions",
            post(react_comment_route),
        )
}

pub(crate) fn comment_to_output(
    comment: &crate::db::comments::CommentRow,
    viewer_id: Uuid,
) -> CommentOutput {
    let (reactions, other_reaction_count) = comment_output(comment, viewer_id);
    CommentOutput {
        id: comment.id,
        workspace_id: comment.workspace_id,
        document_id: comment.document_id,
        task_id: comment.task_id,
        parent_id: comment.parent_id,
        created_by: comment.created_by,
        body: comment.body.clone(),
        resolved_at: comment.resolved_at,
        reactions: reactions
            .into_iter()
            .map(|(emoji, summary)| {
                (
                    emoji,
                    CommentReactionSummary {
                        count: summary.count,
                        reacted_by_me: summary.reacted_by_me,
                    },
                )
            })
            .collect(),
        other_reaction_count,
        created_at: comment.created_at,
        updated_at: comment.updated_at,
    }
}

fn validate_create_body(
    body: &CreateCommentBody,
) -> Result<(Vec<Uuid>, Vec<Uuid>), CommentApiError> {
    Ok((
        body.mentioned_user_ids.clone().unwrap_or_default(),
        body.mentioned_group_ids.clone().unwrap_or_default(),
    ))
}

fn create_input<'a>(
    body: &'a str,
    parent_id: Option<Uuid>,
    mentioned_user_ids: &'a [Uuid],
    mentioned_group_ids: &'a [Uuid],
) -> CreateCommentInput<'a> {
    CreateCommentInput {
        body,
        parent_id,
        mentioned_user_ids,
        mentioned_group_ids,
    }
}

async fn list_document_comments_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    query: Result<Query<CommentListQueryParams>, QueryRejection>,
) -> Result<Json<CommentListResponse>, CommentApiError> {
    let Query(query) = query.map_err(AppError::from)?;
    let auth = require_comment_auth(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::DocumentsRead),
        Some(workspace_id),
    )
    .await?;
    let page = list_document_comments(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        document_id,
        CommentListQuery {
            limit: query.limit.unwrap_or(50),
            cursor: query.cursor,
        },
    )
    .await
    .map_err(internal)?;
    match page {
        Ok(page) => Ok(Json(CommentListResponse {
            items: page
                .items
                .iter()
                .map(|row| comment_to_output(row, auth.user_id))
                .collect(),
            next_cursor: page.next_cursor,
        })),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn list_project_document_comments_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
    query: Result<Query<CommentListQueryParams>, QueryRejection>,
) -> Result<Json<CommentListResponse>, CommentApiError> {
    let Query(query) = query.map_err(AppError::from)?;
    let auth = require_comment_auth(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::DocumentsRead),
        Some(workspace_id),
    )
    .await?;
    let page = list_project_document_comments(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/comments.rs")
            .map_err(internal)?,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        project_id,
        document_id,
        CommentListQuery {
            limit: query.limit.unwrap_or(50),
            cursor: query.cursor,
        },
    )
    .await
    .map_err(internal)?;
    match page {
        Ok(page) => Ok(Json(CommentListResponse {
            items: page
                .items
                .iter()
                .map(|row| comment_to_output(row, auth.user_id))
                .collect(),
            next_cursor: page.next_cursor,
        })),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn list_task_comments_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
    query: Result<Query<CommentListQueryParams>, QueryRejection>,
) -> Result<Json<CommentListResponse>, CommentApiError> {
    let Query(query) = query.map_err(AppError::from)?;
    let auth = require_comment_auth(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    let page = list_task_comments(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/comments.rs")
            .map_err(internal)?,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        task_id,
        CommentListQuery {
            limit: query.limit.unwrap_or(50),
            cursor: query.cursor,
        },
    )
    .await
    .map_err(internal)?;
    match page {
        Ok(page) => Ok(Json(CommentListResponse {
            items: page
                .items
                .iter()
                .map(|row| comment_to_output(row, auth.user_id))
                .collect(),
            next_cursor: page.next_cursor,
        })),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn create_document_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<CreateCommentBody>, JsonRejection>,
) -> Result<Response, CommentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (mentioned_user_ids, mentioned_group_ids) = validate_create_body(&body)?;
    let auth = require_comment_auth(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("comment-create:{}", auth.user_id), 60)
        .await
    {
        return Err(AppError::rate_limited(retry_after).into());
    }
    let created = create_document_comment(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/comments.rs")
            .map_err(internal)?,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        document_id,
        create_input(
            &body.body,
            body.parent_id,
            &mentioned_user_ids,
            &mentioned_group_ids,
        ),
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match created {
        Ok(row) => Ok((
            StatusCode::CREATED,
            Json(comment_to_output(&row, auth.user_id)),
        )
            .into_response()),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn create_project_document_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Result<Json<CreateCommentBody>, JsonRejection>,
) -> Result<Response, CommentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (mentioned_user_ids, mentioned_group_ids) = validate_create_body(&body)?;
    let auth = require_comment_auth(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("comment-create:{}", auth.user_id), 60)
        .await
    {
        return Err(AppError::rate_limited(retry_after).into());
    }
    let created = create_project_document_comment(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/comments.rs")
            .map_err(internal)?,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        (project_id, document_id),
        create_input(
            &body.body,
            body.parent_id,
            &mentioned_user_ids,
            &mentioned_group_ids,
        ),
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match created {
        Ok(row) => Ok((
            StatusCode::CREATED,
            Json(comment_to_output(&row, auth.user_id)),
        )
            .into_response()),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn create_task_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<CreateCommentBody>, JsonRejection>,
) -> Result<Response, CommentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (mentioned_user_ids, mentioned_group_ids) = validate_create_body(&body)?;
    let auth = require_comment_auth(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("comment-create:{}", auth.user_id), 60)
        .await
    {
        return Err(AppError::rate_limited(retry_after).into());
    }
    let created = create_task_comment(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/comments.rs")
            .map_err(internal)?,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        task_id,
        create_input(
            &body.body,
            body.parent_id,
            &mentioned_user_ids,
            &mentioned_group_ids,
        ),
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match created {
        Ok(row) => Ok((
            StatusCode::CREATED,
            Json(comment_to_output(&row, auth.user_id)),
        )
            .into_response()),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn patch_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, comment_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<PatchCommentBody>, JsonRejection>,
) -> Result<Json<CommentOutput>, CommentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    if body.body.is_none() {
        return Err(CommentApiError::App(AppError::from_code(
            ProblemCode::InvalidInput,
        )));
    }
    let auth = require_mutation_auth(&state, &headers, &jar, workspace_id, comment_id).await?;
    let ip = peer_ip(peer.ip());
    let updated = update_comment(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/comments.rs")
            .map_err(internal)?,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        comment_id,
        PatchCommentInput {
            body: body.body.as_deref(),
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match updated {
        Ok(row) => Ok(Json(comment_to_output(&row, auth.user_id))),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn delete_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, comment_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, CommentApiError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = require_mutation_auth(&state, &headers, &jar, workspace_id, comment_id).await?;
    let ip = peer_ip(peer.ip());
    let result = purge_comment(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/comments.rs")
            .map_err(internal)?,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        comment_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn resolve_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, comment_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<CommentOutput>, CommentApiError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = require_mutation_auth(&state, &headers, &jar, workspace_id, comment_id).await?;
    let ip = peer_ip(peer.ip());
    let updated = resolve_comment(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/comments.rs")
            .map_err(internal)?,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        comment_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match updated {
        Ok(row) => Ok(Json(comment_to_output(&row, auth.user_id))),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn unresolve_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, comment_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<CommentOutput>, CommentApiError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = require_mutation_auth(&state, &headers, &jar, workspace_id, comment_id).await?;
    let ip = peer_ip(peer.ip());
    let updated = unresolve_comment(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/comments.rs")
            .map_err(internal)?,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        comment_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match updated {
        Ok(row) => Ok(Json(comment_to_output(&row, auth.user_id))),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn react_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, comment_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<CommentReactionBody>, JsonRejection>,
) -> Result<Json<CommentOutput>, CommentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let auth = require_mutation_auth(&state, &headers, &jar, workspace_id, comment_id).await?;
    let ip = peer_ip(peer.ip());
    let updated = set_comment_reaction(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/comments.rs")
            .map_err(internal)?,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        comment_id,
        ReactionInput {
            emoji: &body.emoji,
            on: body.on,
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match updated {
        Ok(row) => Ok(Json(comment_to_output(&row, auth.user_id))),
        Err(err) => Err(map_comment_error(err)),
    }
}

enum CommentApiError {
    App(AppError),
    Coded {
        status: StatusCode,
        code: &'static str,
        title: String,
        params: Option<serde_json::Value>,
    },
}

impl From<AppError> for CommentApiError {
    fn from(value: AppError) -> Self {
        Self::App(value)
    }
}

impl IntoResponse for CommentApiError {
    fn into_response(self) -> Response {
        match self {
            Self::App(err) => err.into_response(),
            Self::Coded {
                status,
                code,
                title,
                params,
            } => {
                let mut body = json!({
                    "type": "about:blank",
                    "title": title,
                    "status": status.as_u16(),
                    "code": code,
                });
                if let Some(params) = params {
                    body["params"] = params;
                }
                let mut headers = HeaderMap::new();
                headers.insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/problem+json"),
                );
                (status, headers, Json(body)).into_response()
            }
        }
    }
}

fn map_comment_error(err: CommentDbError) -> CommentApiError {
    match err {
        CommentDbError::NotFound => CommentApiError::Coded {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            title: "not found".to_string(),
            params: None,
        },
        CommentDbError::InvalidInput => AppError::from_code(ProblemCode::InvalidInput).into(),
        CommentDbError::InvalidCursor => CommentApiError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_cursor",
            title: "invalid cursor".to_string(),
            params: Some(json!({"code": "invalid_cursor"})),
        },
        CommentDbError::Conflict => CommentApiError::Coded {
            status: StatusCode::CONFLICT,
            code: "conflict",
            title: "conflict".to_string(),
            params: None,
        },
        CommentDbError::ProjectArchived => AppError::from_code(ProblemCode::ProjectArchived).into(),
        CommentDbError::TaskArchived => CommentApiError::Coded {
            status: StatusCode::CONFLICT,
            code: "task_archived",
            title: "task archived".to_string(),
            params: None,
        },
    }
}

async fn require_comment_auth(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    access: Access,
    workspace_id: Option<Uuid>,
) -> Result<RequestAuth, CommentApiError> {
    Ok(authz::require_request_auth(state, headers, jar, access, workspace_id).await?)
}

async fn require_mutation_auth(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    comment_id: Uuid,
) -> Result<RequestAuth, CommentApiError> {
    let auth = require_comment_auth(state, headers, jar, Access::Any, Some(workspace_id)).await?;
    if auth.token_scopes.is_some() {
        let kind = comment_write_kind(
            state
                .auth
                .db
                .pool
                .postgres("src/http/routes/comments.rs")
                .map_err(internal)?,
            workspace_id,
            auth.user_id,
            auth.credential_id,
            comment_id,
        )
        .await
        .map_err(internal)?;
        let kind = kind.map_err(map_comment_error)?;
        let required = match kind {
            CommentWriteKind::Document => ApiTokenScope::DocumentsWrite,
            CommentWriteKind::Task => ApiTokenScope::TasksWrite,
        };
        let scopes = auth.token_scopes.as_deref().unwrap_or(&[]);
        if !grants_api_token_scope(scopes, required) {
            return Err(map_comment_error(CommentDbError::NotFound));
        }
    }
    Ok(auth)
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_wiki_auxiliary_http_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use crate::db::backend::Backend;
    use serde_json::Value;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn state(f: &Fixture) -> AppState {
        AppState {
            auth:Arc::new(crate::auth::AuthService{db:crate::db::Db::from_backend(f.backend.clone()),password_keys:crate::auth::password::Keyring::parse(r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,"test").unwrap()}),
            branding_name:"FVOCI".into(),public_origin:"http://localhost".into(),cookie_secure:false,rate_limiter:crate::http::rate_limit::RateLimiter::new(),storage:crate::attachments::ObjectStorage::local(f.root.join("wiki-read-http-storage")),
            upload:crate::attachments::UploadLimits{part_size_bytes:24,max_file_size_bytes:1024,create_rate_per_5min:20,part_put_slots:crate::attachments::PartPutSlots::new(2)},
            collab:None,meili:None,search_embedder:None,markdown:None,import_wake:None,import_extractor_available:false,preview_extract:None,quota:Default::default(),mailer:Arc::new(crate::mail::Mailer::disabled()),streams:AppState::fresh_streams(),
        }
    }
    async fn get(
        app: Router,
        path: &str,
        cookie: Option<&str>,
        bearer: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = axum::http::Request::builder().uri(path);
        if let Some(cookie) = cookie {
            request = request.header("cookie", format!("fvoci_session={cookie}"));
        }
        if let Some(bearer) = bearer {
            request = request.header("authorization", format!("Bearer {bearer}"));
        }
        let response = app
            .oneshot(request.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
    async fn seed(f: &Fixture) -> (Uuid, Uuid, Uuid, Uuid) {
        let tag = Uuid::now_v7();
        let comment = Uuid::now_v7();
        let project = Uuid::now_v7();
        let workflow = Uuid::now_v7();
        let status = Uuid::now_v7();
        let task = Uuid::now_v7();
        sqlx::query("INSERT INTO document_tags(id,workspace_id,name,color) VALUES(?1,?2,'태그 中 😀','violet')").bind(tag.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO document_tag_assignments(workspace_id,document_id,tag_id) VALUES(?1,?2,?3)").bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(tag.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO comments(id,workspace_id,document_id,created_by,body,reactions) VALUES(?1,?2,?3,?4,'실제 댓글 😀',?5)").bind(comment.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(serde_json::json!({"👍":[f.user]}).to_string()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'AUX','실제 프로젝트','workspace',?3)").bind(project.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO workflows(id,workspace_id,project_id) VALUES(?1,?2,?3)")
            .bind(workflow.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO statuses(id,workspace_id,project_id,workflow_id,name,category,sort_key) VALUES(?1,?2,?3,?4,'Todo','todo','V')").bind(status.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(workflow.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO tasks(id,workspace_id,project_id,number,title,status_id,created_by,content_json) VALUES(?1,?2,?3,17,'실제 작업 😀',?4,?5,?6)").bind(task.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(status.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(serde_json::json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"source-block-17"},"content":[{"type":"text","text":"실제 작업 본문 😀"}]}]}).to_string()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO task_origins(workspace_id,task_id,document_id,request_id,request_hash,anchor) VALUES(?1,?2,?3,?4,'literal-read-http','source-block-17')").bind(f.workspace.as_bytes().as_slice()).bind(task.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(Uuid::now_v7().as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        (tag, comment, project, task)
    }

    #[tokio::test]
    async fn wiki_aux_http_selected_four_nonempty_gets_pat_scopes_current_auth_and_tenant() {
        let f = Fixture::new().await;
        let (tag, comment, project, task) = seed(&f).await;
        let token = crate::auth::token::new_token();
        let credential = Uuid::now_v7();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                credential,
                f.user,
                &token.hash,
                chrono::DateTime::from_timestamp_micros(
                    chrono::Utc::now().timestamp_micros() + 86_400_000_000,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let app = router()
            .merge(crate::http::routes::document_tags::router())
            .merge(crate::http::routes::task_body::router())
            .with_state(state(&f));
        let base = format!(
            "/api/v1/workspaces/{}/documents/{}",
            f.workspace, f.document
        );
        let paths = [
            format!("{base}/tags"),
            format!("{base}/task-origins?limit=50"),
            format!("{base}/task-projects"),
            format!("{base}/comments"),
        ];
        for path in &paths {
            assert_eq!(
                get(app.clone(), path, None, None).await.0,
                StatusCode::UNAUTHORIZED
            );
        }
        let (code, tags) = get(app.clone(), &paths[0], Some(&token.token), None).await;
        assert_eq!(code, StatusCode::OK, "{tags}");
        assert_eq!(tags["items"][0]["id"], tag.to_string());
        assert_eq!(tags["items"][0]["name"], "태그 中 😀");
        assert_eq!(tags["items"][0]["color"], "violet");
        assert_eq!(tags["items"].as_array().unwrap().len(), 1);
        let (code, origins) = get(app.clone(), &paths[1], Some(&token.token), None).await;
        assert_eq!(code, StatusCode::OK, "{origins}");
        assert_eq!(origins["count"], 1);
        assert_eq!(origins["items"][0]["taskId"], task.to_string());
        assert_eq!(origins["items"][0]["documentId"], f.document.to_string());
        assert_eq!(origins["items"][0]["taskDisplayId"], "AUX-17");
        assert_eq!(origins["items"][0]["taskTitle"], "실제 작업 😀");
        assert_eq!(origins["items"][0]["anchor"], "source-block-17");
        assert!(origins["nextCursor"].is_null());
        let (code, picker) = get(app.clone(), &paths[2], Some(&token.token), None).await;
        assert_eq!(code, StatusCode::OK, "{picker}");
        assert_eq!(picker["items"][0]["id"], project.to_string());
        assert_eq!(picker["suggestedId"], project.to_string());
        assert_eq!(picker["canCreateProject"], true);
        assert_eq!(picker["items"].as_array().unwrap().len(), 1);
        let (code, comments) = get(app.clone(), &paths[3], Some(&token.token), None).await;
        assert_eq!(code, StatusCode::OK, "{comments}");
        assert_eq!(comments["items"][0]["id"], comment.to_string());
        assert_eq!(comments["items"][0]["body"], "실제 댓글 😀");
        assert_eq!(comments["items"][0]["createdBy"], f.user.to_string());
        assert_eq!(comments["items"][0]["reactions"]["👍"]["count"], 1);
        assert_eq!(comments["items"][0]["reactions"]["👍"]["reactedByMe"], true);
        assert_eq!(comments["items"][0]["otherReactionCount"], 0);
        assert!(comments["nextCursor"].is_null());
        let pat = crate::auth::token::new_token();
        let pat_id = Uuid::now_v7();
        sqlx::query("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes) VALUES(?1,?2,?3,?4,'wiki reads','[\"documents.read\"]')").bind(pat_id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(&pat.hash).execute(&f.pool).await.unwrap();
        for index in [0, 2, 3] {
            assert_eq!(
                get(app.clone(), &paths[index], None, Some(&pat.token))
                    .await
                    .0,
                StatusCode::OK
            );
        }
        assert_eq!(
            get(app.clone(), &paths[1], None, Some(&pat.token)).await.0,
            StatusCode::NOT_FOUND
        );
        sqlx::query(
            "UPDATE api_tokens SET scopes='[\"documents.read\",\"tasks.read\"]' WHERE id=?1",
        )
        .bind(pat_id.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        assert_eq!(
            get(app.clone(), &paths[1], None, Some(&pat.token)).await.0,
            StatusCode::OK
        );
        for suffix in [
            "task-origins?limit=0",
            "task-origins?limit=101",
            "comments?limit=0",
            "comments?cursor=wrong",
        ] {
            assert_eq!(
                get(
                    app.clone(),
                    &format!("{base}/{suffix}"),
                    Some(&token.token),
                    None
                )
                .await
                .0,
                StatusCode::BAD_REQUEST
            );
        }
        sqlx::query("UPDATE memberships SET role='guest' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for path in &paths {
            assert_eq!(
                get(app.clone(), path, Some(&token.token), None).await.0,
                StatusCode::NOT_FOUND
            );
        }
        sqlx::query("UPDATE memberships SET role='owner' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET suspended_at=1 WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for path in &paths {
            assert_eq!(
                get(app.clone(), path, Some(&token.token), None).await.0,
                StatusCode::UNAUTHORIZED
            );
        }
        sqlx::query("UPDATE users SET suspended_at=NULL WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE workspaces SET deleted_at=1 WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for path in &paths {
            assert_eq!(
                get(app.clone(), path, Some(&token.token), None).await.0,
                StatusCode::NOT_FOUND
            );
        }
        sqlx::query("UPDATE workspaces SET deleted_at=NULL WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE documents SET deleted_at=1 WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for path in &paths {
            assert_eq!(
                get(app.clone(), path, Some(&token.token), None).await.0,
                StatusCode::NOT_FOUND
            );
        }
        sqlx::query("UPDATE documents SET deleted_at=NULL WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE comments RENAME TO comments_http_failure")
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            get(app.clone(), &paths[3], Some(&token.token), None)
                .await
                .0,
            StatusCode::INTERNAL_SERVER_ERROR
        );
        sqlx::query("ALTER TABLE comments_http_failure RENAME TO comments")
            .execute(&f.pool)
            .await
            .unwrap();
        let other = Uuid::now_v7();
        let doc = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,'wiki-http-other','Other')")
            .bind(other.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')")
            .bind(other.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'Other',?3,'V',1,'published',2,?4,'{}')").bind(doc.as_bytes().as_slice()).bind(other.as_bytes().as_slice()).bind(doc.simple().to_string()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO comments(id,workspace_id,document_id,created_by,body) VALUES(?1,?2,?3,?4,'Other tenant literal')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(other.as_bytes().as_slice()).bind(doc.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let other_base = format!("/api/v1/workspaces/{other}/documents/{doc}");
        let (code, other_page) = get(
            app.clone(),
            &format!("{other_base}/comments"),
            Some(&token.token),
            None,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(other_page["items"][0]["body"], "Other tenant literal");
        assert_eq!(
            get(
                app.clone(),
                &format!("{other_base}/comments"),
                None,
                Some(&pat.token)
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        for suffix in ["tags", "task-origins?limit=50", "task-projects", "comments"] {
            assert_eq!(
                get(
                    app.clone(),
                    &format!(
                        "/api/v1/workspaces/{other}/documents/{}/{suffix}",
                        f.document
                    ),
                    Some(&token.token),
                    None
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
        }
        sqlx::query("UPDATE documents SET project_id=?1 WHERE id=?2")
            .bind(project.as_bytes().as_slice())
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for index in [0, 3] {
            assert_eq!(
                get(app.clone(), &paths[index], Some(&token.token), None)
                    .await
                    .0,
                StatusCode::NOT_FOUND
            );
        }
        sqlx::query("UPDATE documents SET project_id=NULL WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for path in &paths {
            assert_eq!(
                get(app.clone(), path, Some(&token.token), Some(&pat.token))
                    .await
                    .0,
                StatusCode::UNAUTHORIZED
            );
        }
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        // Fresh connection and credential use the actual current API adapter;
        // this is readback, never proof about a remote original stream finish.
        let fresh = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let mut fresh_state = state(&f);
        fresh_state.auth = Arc::new(crate::auth::AuthService {
            db: crate::db::Db::from_backend(Backend::Sqlite(fresh.clone())),
            password_keys: crate::auth::password::Keyring::parse(
                r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
                "test",
            )
            .unwrap(),
        });
        let fresh_app = router()
            .merge(crate::http::routes::document_tags::router())
            .merge(crate::http::routes::task_body::router())
            .with_state(fresh_state);
        let expected_ids = [tag, task, project, comment];
        for (index, path) in paths.iter().enumerate() {
            let (code, page) = get(fresh_app.clone(), path, None, Some(&pat.token)).await;
            assert_eq!(code, StatusCode::OK, "{page}");
            let field = if index == 1 { "taskId" } else { "id" };
            assert_eq!(page["items"][0][field], expected_ids[index].to_string());
            assert_eq!(page["items"].as_array().unwrap().len(), 1);
        }
        drop(fresh_app);
        drop(app);
        fresh.close().await;
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }
}
