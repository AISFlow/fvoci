//! Task body block patch and task origins (source
//! `apps/server/src/domains/tasks/routes.ts` `patchBlock`,
//! `apps/server/src/domains/documents/task-origins.ts`).
//!
//! Tasks have no body GET/PUT: the body is read through `contentJson` of
//! `GET tasks/{id}` and written only through the task collab room (editor,
//! block patch, revision restore). The block patch is a read-modify-write of
//! the live projection that the room actor applies as one forward update
//! carrying the projected tail, like the document block patch.

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::documents_dto::PatchBlockInput;
use crate::api::dto::{CreateTaskBody, TaskMetaOutput};
use crate::api::tasks_dto::{
    DocumentTaskCreateBody, DocumentTaskCreateOutput, TaskOriginItemOutput, TaskOriginListResponse,
    TaskProjectOutput, TaskProjectPickerResponse,
};
use crate::auth::scopes::{grants_api_token_scope, ApiTokenScope};
use crate::collab::derived_body::{prepare_derived_body, DerivedBodyError};
use crate::collab::room::{BodyWriteError, RoomKey};
use crate::collab::seed::{SeedEngine, SeedError};
use crate::db::revisions::{
    authorize_revision_target_backend as authorize_revision_target, RevisionDbError, RevisionTarget,
};
use crate::db::task_origins::{
    create_document_task_backend as create_document_task,
    get_task_origin_backend as get_task_origin,
    list_document_task_origins_backend as list_document_task_origins, origin_request_hash,
    task_projects_backend as task_projects, DocumentTaskRequest, TaskOriginDbError,
    TASK_ORIGIN_ANCHOR_MAX_CHARS,
};
use crate::db::tasks::{get_task_backend as get_task, CreateTaskInput};
use crate::documents::blocks::{replace_node_by_id, BlockNode};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access, RequestAuth};
use crate::http::guard::check_origin;
use crate::http::rate_limit::{peer_ip, REVISION_WRITE_LIMIT, REVISION_WRITE_WINDOW};
use crate::http::routes::tasks::{
    activity_channel, internal, map_task_db_error, task_meta_output, TaskApiError,
};
use crate::http::state::AppState;
use crate::tasks::{priority_is_valid, task_type_is_valid, title_is_valid};

/// Attempts of the block read-modify-write when a concurrent edit moves the tail.
const PATCH_BLOCK_ATTEMPTS: usize = 3;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/blocks/{block_id}",
            patch(patch_task_block),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/origin",
            get(task_origin),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/tasks",
            post(create_task_from_document),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/task-projects",
            get(document_task_projects),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/task-origins",
            get(document_task_origins),
        )
}

fn coded(status: StatusCode, code: &'static str, title: &str) -> TaskApiError {
    TaskApiError::Coded {
        status,
        code,
        title: title.to_string(),
    }
}

fn too_large() -> TaskApiError {
    coded(
        StatusCode::PAYLOAD_TOO_LARGE,
        "document_body_exceeds_document_max_body_bytes",
        "document body exceeds document max body bytes",
    )
}

fn invalid_body() -> TaskApiError {
    coded(
        StatusCode::BAD_REQUEST,
        "invalid_document_body",
        "invalid document body",
    )
}

fn collab_unavailable() -> TaskApiError {
    coded(
        StatusCode::SERVICE_UNAVAILABLE,
        "collab_unavailable",
        "collab unavailable",
    )
}

fn map_body_write(err: BodyWriteError) -> TaskApiError {
    match err {
        BodyWriteError::Rejected => AppError::from_code(ProblemCode::NotFound).into(),
        BodyWriteError::Unavailable => collab_unavailable(),
        BodyWriteError::Conflict => coded(
            StatusCode::CONFLICT,
            "document_version_mismatch",
            "document version mismatch",
        ),
        BodyWriteError::TooLarge => too_large(),
        BodyWriteError::Invalid => invalid_body(),
        BodyWriteError::DeriveFailed => {
            tracing::error!("task body write committed but the derived body failed");
            AppError::internal().into()
        }
    }
}

fn map_seed(err: SeedError) -> TaskApiError {
    match err {
        SeedError::InvalidInput(detail) => {
            tracing::info!(%detail, "task body seed refused");
            invalid_body()
        }
        SeedError::TooLarge(_) => too_large(),
        SeedError::Unavailable => collab_unavailable(),
        SeedError::Failed(detail) => {
            tracing::error!(error = %detail, "task body seed failed");
            AppError::internal().into()
        }
    }
}

fn map_derived(err: DerivedBodyError) -> TaskApiError {
    match err {
        DerivedBodyError::TooLarge => too_large(),
        DerivedBodyError::InvalidDocumentBody(_) => invalid_body(),
    }
}

fn map_revision_access(err: RevisionDbError) -> TaskApiError {
    match err {
        RevisionDbError::NotFound
        | RevisionDbError::Forbidden
        | RevisionDbError::StaleRevisionHead => AppError::from_code(ProblemCode::NotFound).into(),
        RevisionDbError::RestoreConflict => AppError::internal().into(),
        RevisionDbError::TaskArchived => AppError::from_code(ProblemCode::TaskArchived).into(),
        RevisionDbError::ProjectArchived => {
            AppError::from_code(ProblemCode::ProjectArchived).into()
        }
    }
}

async fn auth(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    scope: ApiTokenScope,
    workspace_id: Uuid,
) -> Result<RequestAuth, AppError> {
    require_request_auth(
        state,
        headers,
        jar,
        Access::Scope(scope),
        Some(workspace_id),
    )
    .await
}

/// Source `requireApiTokenScope`: an API token also needs this second scope.
fn require_extra_scope(auth: &RequestAuth, scope: ApiTokenScope) -> Result<(), AppError> {
    match auth.token_scopes.as_deref() {
        Some(scopes) if !grants_api_token_scope(scopes, scope) => {
            Err(AppError::from_code(ProblemCode::NotFound))
        }
        _ => Ok(()),
    }
}

/// Source `onRevisionWriteLimit`: body, block and revision writes share one budget.
async fn revision_write_limit(state: &AppState, user_id: Uuid) -> Result<(), AppError> {
    state
        .rate_limiter
        .allow_window(
            &format!("revision-write:{user_id}"),
            REVISION_WRITE_LIMIT,
            REVISION_WRITE_WINDOW,
        )
        .await
        .map_err(AppError::rate_limited)
}

// ---------------------------------------------------------------------------
// PATCH tasks/{id}/blocks/{blockId}

fn parse_block_input(bytes: &[u8], block_id: &str) -> Result<BlockNode, TaskApiError> {
    let input: PatchBlockInput = serde_json::from_slice(bytes)
        .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    if input.r#type.is_empty() {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    // Source `patchCollabBlock`: an explicit `attrs.id` must name the patched block.
    if let Some(id) = input.attrs.as_ref().and_then(|attrs| attrs.get("id")) {
        if id.as_str() != Some(block_id) {
            return Err(invalid_body());
        }
    }
    Ok(BlockNode {
        r#type: input.r#type,
        attrs: input.attrs,
        content: input.content,
        marks: input.marks,
        text: input.text,
    })
}

async fn patch_task_block(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id, block_id)): Path<(Uuid, Uuid, String)>,
    body: Bytes,
) -> Result<Json<TaskMetaOutput>, TaskApiError> {
    check_origin(&headers, &state.public_origin)?;
    let node = parse_block_input(&body, &block_id)?;
    let auth = auth(
        &state,
        &headers,
        &jar,
        ApiTokenScope::TasksWrite,
        workspace_id,
    )
    .await?;
    revision_write_limit(&state, auth.user_id).await?;
    // Trashed → 404, no edit → 404, archived task/project → 409 (`assertTaskWritable`).
    authorize_revision_target(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        RevisionTarget::Task(task_id),
        true,
    )
    .await
    .map_err(internal)?
    .map_err(map_revision_access)?;
    let Some(hub) = state.collab.clone() else {
        return Err(collab_unavailable());
    };
    let key = RoomKey::task(workspace_id, task_id);
    let timeout = hub.rpc_timeout().max(Duration::from_millis(1));
    let mut attempt = 0;
    loop {
        attempt += 1;
        let live = match tokio::time::timeout(
            timeout,
            hub.project_live(key, auth.user_id, auth.credential_id),
        )
        .await
        {
            Ok(result) => result.map_err(map_body_write)?,
            Err(_) => return Err(AppError::from_code(ProblemCode::CollabTimeoutRetry).into()),
        };
        let Some(patched) = replace_node_by_id(&live.content_json, &block_id, &node) else {
            return Err(AppError::from_code(ProblemCode::NotFound).into());
        };
        prepare_derived_body(patched.clone()).map_err(map_derived)?;
        let seed = SeedEngine::from_hub(&hub)
            .tiptap_to_yjs_update(&patched)
            .await
            .map_err(map_seed)?;
        let write = hub.replace_body(
            key,
            auth.user_id,
            auth.credential_id,
            seed,
            Some(live.tail_seq),
        );
        match tokio::time::timeout(timeout, write).await {
            Ok(Ok(())) => break,
            Ok(Err(BodyWriteError::Conflict)) if attempt < PATCH_BLOCK_ATTEMPTS => continue,
            Ok(Err(err)) => return Err(map_body_write(err)),
            Err(_) => return Err(AppError::from_code(ProblemCode::CollabTimeoutRetry).into()),
        }
    }
    let task = get_task(
        &state.auth.db.pool,
        workspace_id,
        task_id,
        auth.user_id,
        auth.credential_id,
    )
    .await
    .map_err(internal)?
    .map_err(map_task_db_error)?;
    Ok(Json(task_meta_output(task.meta)))
}

// ---------------------------------------------------------------------------
// task origins

pub(crate) fn map_origin_error(err: TaskOriginDbError) -> TaskApiError {
    match err {
        TaskOriginDbError::NotFound | TaskOriginDbError::Forbidden => {
            AppError::from_code(ProblemCode::NotFound).into()
        }
        TaskOriginDbError::RequestMismatch => coded(
            StatusCode::CONFLICT,
            "document_version_mismatch",
            "document version mismatch",
        ),
        TaskOriginDbError::Task(err) => map_task_db_error(err),
    }
}

/// Source `taskCreateInput` after parsing, as hashed by `createDocumentTask`.
pub(crate) fn normalized_task_input(body: &CreateTaskBody) -> Value {
    json!({
        "title": body.title,
        "type": body.task_type,
        "priority": body.priority,
        "statusId": body.status_id,
        "startDate": body.start_date,
        "dueDate": body.due_date,
        "parentId": body.parent_id,
        "milestoneId": body.milestone_id,
        "recurrence": body.recurrence,
    })
}

async fn create_task_from_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    body: Bytes,
) -> Result<Response, TaskApiError> {
    check_origin(&headers, &state.public_origin)?;
    let input: DocumentTaskCreateBody = serde_json::from_slice(&body)
        .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    if input
        .anchor
        .as_deref()
        .is_some_and(|anchor| anchor.chars().count() > TASK_ORIGIN_ANCHOR_MAX_CHARS)
    {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let task: CreateTaskBody = serde_json::from_value(input.task.clone())
        .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    if !title_is_valid(&task.title)
        || !task_type_is_valid(&task.task_type)
        || !priority_is_valid(&task.priority)
    {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let auth = auth(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
    )
    .await?;
    require_extra_scope(&auth, ApiTokenScope::TasksWrite)?;
    let mut normalized = normalized_task_input(&task);
    if input.self_assign {
        normalized["selfAssign"] = Value::Bool(true);
    }
    let request_hash = origin_request_hash(
        auth.user_id,
        input.project_id,
        input.anchor.as_deref(),
        &normalized,
    );
    let ip = peer_ip(peer.ip());
    let outcome = create_document_task(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        DocumentTaskRequest {
            document_id,
            project_id: input.project_id,
            request_id: input.request_id,
            self_assign: input.self_assign,
            anchor: input.anchor.as_deref(),
            request_hash: &request_hash,
            task: CreateTaskInput {
                title: &task.title,
                task_type: &task.task_type,
                priority: &task.priority,
                status_id: task.status_id,
                start_date: task.start_date,
                due_date: task.due_date,
                parent_id: task.parent_id,
                milestone_id: task.milestone_id,
                recurrence: task.recurrence.clone(),
            },
        },
        Some(&ip),
        activity_channel(&headers),
    )
    .await
    .map_err(internal)?
    .map_err(map_origin_error)?;
    // Source always answers 201, replays included.
    Ok((
        StatusCode::CREATED,
        Json(DocumentTaskCreateOutput {
            task_id: outcome.task_id().to_string(),
        }),
    )
        .into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginQuery {
    after: Option<Uuid>,
    limit: Option<i64>,
}

async fn document_task_projects(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<TaskProjectPickerResponse>, TaskApiError> {
    let auth = auth(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
    )
    .await?;
    let picker = task_projects(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        document_id,
    )
    .await
    .map_err(internal)?
    .map_err(map_origin_error)?;
    Ok(Json(TaskProjectPickerResponse {
        items: picker
            .items
            .into_iter()
            .map(|item| TaskProjectOutput {
                id: item.id.to_string(),
                name: item.name,
                key: item.key,
                visibility: item.visibility,
            })
            .collect(),
        suggested_id: picker.suggested_id.map(|id| id.to_string()),
        can_create_project: picker.can_create_project,
    }))
}

async fn document_task_origins(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    query: Result<Query<OriginQuery>, QueryRejection>,
) -> Result<Json<TaskOriginListResponse>, TaskApiError> {
    let Query(query) = query.map_err(AppError::from)?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let auth = auth(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
    )
    .await?;
    require_extra_scope(&auth, ApiTokenScope::TasksRead)?;
    let page = list_document_task_origins(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        document_id,
        query.after,
        limit,
    )
    .await
    .map_err(internal)?
    .map_err(map_origin_error)?;
    Ok(Json(origin_response(page)))
}

fn origin_response(page: crate::db::task_origins::TaskOriginPage) -> TaskOriginListResponse {
    TaskOriginListResponse {
        count: page.count,
        items: page
            .items
            .into_iter()
            .map(|item| TaskOriginItemOutput {
                task_id: item.task_id.to_string(),
                document_id: item.document_id.to_string(),
                task_display_id: item.task_display_id,
                document_display_id: item.document_display_id,
                task_title: item.task_title,
                document_title: item.document_title,
                anchor: item.anchor,
            })
            .collect(),
        next_cursor: page.next_cursor.map(|id| id.to_string()),
    }
}

async fn task_origin(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
    query: Result<Query<OriginQuery>, QueryRejection>,
) -> Result<Json<TaskOriginListResponse>, TaskApiError> {
    let Query(query) = query.map_err(AppError::from)?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let auth = auth(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
    )
    .await?;
    require_extra_scope(&auth, ApiTokenScope::TasksRead)?;
    let page = get_task_origin(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        task_id,
        query.after,
        limit,
    )
    .await
    .map_err(internal)?
    .map_err(map_origin_error)?;
    Ok(Json(origin_response(page)))
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_task_origin_http_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use crate::db::lookup::selected_lookup_tests::session;
    use crate::db::tasks::selected_task_detail_tests::setup;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn state(f: &Fixture) -> AppState {
        AppState {
            auth:Arc::new(crate::auth::AuthService{db:crate::db::Db::from_backend(f.backend.clone()),password_keys:crate::auth::password::Keyring::parse(r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,"test").unwrap()}),
            branding_name:"FVOCI".into(),public_origin:"http://localhost".into(),cookie_secure:false,rate_limiter:crate::http::rate_limit::RateLimiter::new(),storage:crate::attachments::ObjectStorage::local(f.root.join("task-origin-http-storage")),
            upload:crate::attachments::UploadLimits{part_size_bytes:24,max_file_size_bytes:1024,create_rate_per_5min:20,part_put_slots:crate::attachments::PartPutSlots::new(2)},
            collab:None,meili:None,search_embedder:None,markdown:None,import_wake:None,import_extractor_available:false,preview_extract:None,quota:Default::default(),mailer:Arc::new(crate::mail::Mailer::disabled()),streams:AppState::fresh_streams(),
        }
    }

    async fn call(
        app: Router,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        bearer: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("origin", "http://localhost");
        if let Some(cookie) = cookie {
            request = request.header("cookie", format!("fvoci_session={cookie}"));
        }
        if let Some(token) = bearer {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let payload = if let Some(body) = body {
            request = request.header("content-type", "application/json");
            axum::body::Body::from(body.to_string())
        } else {
            axum::body::Body::empty()
        };
        let mut request = request.body(payload).unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
        let response = app.oneshot(request).await.unwrap();
        let code = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        (code, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn wiki_aux_task_origin_inverse_http_normal_create_replay_literal_scopes_current_denial()
    {
        let (f, credential, cookie, project, _) = setup().await;
        let app = router().with_state(state(&f));
        let create_path = format!(
            "/api/v1/workspaces/{}/documents/{}/tasks",
            f.workspace, f.document
        );
        let command = Uuid::now_v7();
        let input = json!({"projectId":project,"requestId":command,"anchor":"http-origin-block","task":{"title":"HTTP 태스크 中 😀"}});
        let (code, created) = call(
            app.clone(),
            "POST",
            &create_path,
            Some(&cookie),
            None,
            Some(input.clone()),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "{created}");
        let task = Uuid::parse_str(created["taskId"].as_str().unwrap()).unwrap();
        let (code, replay) = call(
            app.clone(),
            "POST",
            &create_path,
            Some(&cookie),
            None,
            Some(input.clone()),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        assert_eq!(replay, created);
        let mut changed = input;
        changed["task"]["title"] = json!("Changed");
        let (code, problem) = call(
            app.clone(),
            "POST",
            &create_path,
            Some(&cookie),
            None,
            Some(changed),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT);
        assert_eq!(problem["code"], "document_version_mismatch");
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM task_origins WHERE workspace_id=?1 AND document_id=?2 AND request_id=?3").bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(command.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(count, 1);
        let path = format!("/api/v1/workspaces/{}/tasks/{task}/origin", f.workspace);
        let expected = json!({"count":1,"items":[{"taskId":task,"documentId":f.document,"taskDisplayId":"ORIGIN-3","documentDisplayId":"WIKI-1","taskTitle":"HTTP 태스크 中 😀","documentTitle":"S31","anchor":"http-origin-block"}],"nextCursor":null});
        let (code, body) = call(app.clone(), "GET", &path, Some(&cookie), None, None).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body, expected);
        for suffix in ["?limit=0", "?limit=101", "?after=invalid", "?limit=bad"] {
            let (code, _) = call(
                app.clone(),
                "GET",
                &format!("{path}{suffix}"),
                Some(&cookie),
                None,
                None,
            )
            .await;
            assert_eq!(code, StatusCode::BAD_REQUEST);
        }
        let (code, body) = call(
            app.clone(),
            "GET",
            &format!("{path}?after={task}&limit=1"),
            Some(&cookie),
            None,
            None,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body, json!({"count":0,"items":[],"nextCursor":null}));
        let mut capable = None;
        for scopes in [
            json!(["documents.read"]),
            json!(["tasks.read"]),
            json!(["documents.read", "tasks.read"]),
        ] {
            let token = crate::auth::token::new_token();
            sqlx::query("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes) VALUES(?1,?2,?3,?4,'Inverse',?5)").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(&token.hash).bind(scopes.to_string()).execute(&f.pool).await.unwrap();
            let (code, body) =
                call(app.clone(), "GET", &path, None, Some(&token.token), None).await;
            if scopes.as_array().unwrap().len() == 2 {
                assert_eq!(code, StatusCode::OK);
                assert_eq!(body, expected);
                capable = Some(token);
            } else {
                assert_eq!(code, StatusCode::NOT_FOUND);
                assert_eq!(body["code"], "not_found");
            }
        }
        let capable = capable.unwrap();
        for cookie in [None, Some("stale")] {
            let (code, body) = call(app.clone(), "GET", &path, cookie, None, None).await;
            assert_eq!(code, StatusCode::UNAUTHORIZED);
            assert_eq!(body["code"], "authentication_required");
        }
        let (code, _) = call(
            app.clone(),
            "GET",
            &path,
            Some("stale"),
            Some(&capable.token),
            None,
        )
        .await;
        assert_eq!(code, StatusCode::UNAUTHORIZED);
        let wrong = format!("/api/v1/workspaces/{}/tasks/{task}/origin", Uuid::now_v7());
        assert_eq!(
            call(app.clone(), "GET", &wrong, None, Some(&capable.token), None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        let other = Uuid::now_v7();
        sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,?2,'Other')")
            .bind(other.as_bytes().as_slice())
            .bind(format!("{other}@example.test"))
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(other.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let (_, other_cookie) = session(&f, other).await;
        assert_eq!(
            call(app.clone(), "GET", &path, Some(&other_cookie), None, None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        sqlx::query("UPDATE memberships SET role='guest' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let (code, hidden) = call(app.clone(), "GET", &path, Some(&cookie), None, None).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(hidden, json!({"count":0,"items":[],"nextCursor":null}));
        sqlx::query("UPDATE memberships SET role='owner' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            call(app.clone(), "GET", &path, Some(&cookie), None, None)
                .await
                .1,
            expected
        );
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            call(app, "GET", &path, Some(&cookie), None, None).await.0,
            StatusCode::UNAUTHORIZED
        );
        f.close().await;
    }

    async fn native_state(f: &Fixture) -> (AppState, Arc<crate::collab::CollabHub>) {
        let config = crate::collab::config::CollabConfig::from_env()
            .expect("root-qualified FVOCI_COLLAB_ENGINE required for actual native route oracle");
        let hub = Arc::new(
            crate::collab::CollabHub::new_backend(
                config,
                f.backend.clone(),
                Some(crate::collab::config::FamilyRoomTimings::new(30_000, 5_000).unwrap()),
            )
            .unwrap(),
        );
        let mut state = state(f);
        state.collab = Some(hub.clone());
        (state, hub)
    }

    #[tokio::test]
    async fn wiki_aux_task_body_patch_native_literal_ids_scopes_archives_and_tail_conflict() {
        let (f, credential, cookie, project, task) = setup().await;
        let admission = crate::db::migrate::SqliteAdmission::server(&f.path).unwrap();
        let (state, hub) = native_state(&f).await;
        let app = router().with_state(state);
        let key = RoomKey::task(f.workspace, task);
        let original = json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"b-1"},"content":[{"type":"text","text":"原 본문 中 😀"}]},{"type":"paragraph","attrs":{"id":"b-2"},"content":[{"type":"text","text":"둘째"}]}]});
        let seed = SeedEngine::from_hub(&hub)
            .tiptap_to_yjs_update(&original)
            .await
            .unwrap();
        hub.replace_body(key, f.user, credential, seed, None)
            .await
            .unwrap();
        let initial =
            crate::db::tasks::get_task_backend(&f.backend, f.workspace, task, f.user, credential)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(initial.content_json["content"][0]["attrs"]["id"], "b-1");
        assert_eq!(
            initial.content_json["content"][0]["content"][0]["text"],
            "原 본문 中 😀"
        );
        let path = format!("/api/v1/workspaces/{}/tasks/{task}/blocks/b-2", f.workspace);
        let patch = json!({"type":"heading","attrs":{"level":2},"content":[{"type":"text","text":"수정 中 😀"}]});
        let (code, meta) = call(
            app.clone(),
            "PATCH",
            &path,
            Some(&cookie),
            None,
            Some(patch.clone()),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{meta}");
        assert_eq!(meta["id"], task.to_string());
        assert_eq!(meta["projectId"], project.to_string());
        let fresh = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let detail = crate::db::tasks::get_task_backend(
            &crate::db::backend::Backend::Sqlite(fresh.clone()),
            f.workspace,
            task,
            f.user,
            credential,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            detail.content_json["content"][0],
            initial.content_json["content"][0]
        );
        assert_eq!(detail.content_json["content"][1]["attrs"]["id"], "b-2");
        assert_eq!(detail.content_json["content"][1]["type"], "heading");
        assert_eq!(
            detail.content_json["content"][1]["content"],
            patch["content"]
        );
        let (code, problem) = call(
            app.clone(),
            "PATCH",
            &path,
            Some(&cookie),
            None,
            Some(json!({"type":"paragraph","attrs":{"id":"other"}})),
        )
        .await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
        assert_eq!(problem["code"], "invalid_document_body");
        let missing = format!(
            "/api/v1/workspaces/{}/tasks/{task}/blocks/nope",
            f.workspace
        );
        assert_eq!(
            call(
                app.clone(),
                "PATCH",
                &missing,
                Some(&cookie),
                None,
                Some(patch.clone())
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        for (scopes, allowed) in [
            (json!(["tasks.read"]), false),
            (json!(["tasks.write"]), true),
        ] {
            let token = crate::auth::token::new_token();
            sqlx::query("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes) VALUES(?1,?2,?3,?4,'Patch',?5)").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(&token.hash).bind(scopes.to_string()).execute(&f.pool).await.unwrap();
            let (code, body) = call(
                app.clone(),
                "PATCH",
                &path,
                None,
                Some(&token.token),
                Some(json!({"type":"paragraph","content":[{"type":"text","text":"PAT 中 😀"}]})),
            )
            .await;
            assert_eq!(
                code,
                if allowed {
                    StatusCode::OK
                } else {
                    StatusCode::NOT_FOUND
                },
                "{body}"
            );
        }
        sqlx::query("UPDATE project_members SET role='viewer' WHERE workspace_id=?1 AND project_id=?2 AND user_id=?3")
            .bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(
            call(
                app.clone(),
                "PATCH",
                &path,
                Some(&cookie),
                None,
                Some(patch.clone())
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        sqlx::query("UPDATE project_members SET role='lead' WHERE workspace_id=?1 AND project_id=?2 AND user_id=?3")
            .bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        for (table, assignment, restore, expected) in [
            (
                "tasks",
                "archived_at=1",
                "archived_at=NULL",
                "task_archived",
            ),
            (
                "projects",
                "status='archived'",
                "status='active'",
                "project_archived",
            ),
        ] {
            let id = if table == "tasks" { task } else { project };
            sqlx::query(&format!("UPDATE {table} SET {assignment} WHERE id=?1"))
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            let (code, body) = call(
                app.clone(),
                "PATCH",
                &path,
                Some(&cookie),
                None,
                Some(patch.clone()),
            )
            .await;
            assert_eq!(code, StatusCode::CONFLICT);
            assert_eq!(body["code"], expected);
            sqlx::query(&format!("UPDATE {table} SET {restore} WHERE id=?1"))
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        let live = hub.project_live(key, f.user, credential).await.unwrap();
        let seed = SeedEngine::from_hub(&hub)
            .tiptap_to_yjs_update(&original)
            .await
            .unwrap();
        assert!(matches!(
            hub.replace_body(key, f.user, credential, seed, Some(live.tail_seq - 1))
                .await,
            Err(BodyWriteError::Conflict)
        ));
        assert!(hub.shutdown().await.is_clean());
        fresh.close().await;
        drop(admission);
        f.close().await;
    }

    #[tokio::test]
    async fn wiki_aux_task_body_patch_native_current_revoke_after_projection_no_publish_then_healthy(
    ) {
        let (f, credential, cookie, project, task) = setup().await;
        let admission = crate::db::migrate::SqliteAdmission::server(&f.path).unwrap();
        let (state, hub) = native_state(&f).await;
        let app = router().with_state(state);
        let key = RoomKey::task(f.workspace, task);
        let original = json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"b-1"},"content":[{"type":"text","text":"原 中 😀"}]}]});
        let seed = SeedEngine::from_hub(&hub)
            .tiptap_to_yjs_update(&original)
            .await
            .unwrap();
        hub.replace_body(key, f.user, credential, seed, None)
            .await
            .unwrap();
        let before =
            crate::db::tasks::get_task_backend(&f.backend, f.workspace, task, f.user, credential)
                .await
                .unwrap()
                .unwrap();
        let history: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM task_collab_updates WHERE workspace_id=?1 AND task_id=?2",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(task.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        let path = format!("/api/v1/workspaces/{}/tasks/{task}/blocks/b-1", f.workspace);
        let patch = json!({"type":"paragraph","content":[{"type":"text","text":"不应发布 中 😀"}]});
        let (reached, proceed) = crate::collab::room::arm_native_consumer_barrier(
            task,
            crate::collab::room::NATIVE_PROJECT_FINAL_PROOF,
        )
        .await;
        let pending = tokio::spawn({
            let app = app.clone();
            let path = path.clone();
            let cookie = cookie.clone();
            let patch = patch.clone();
            async move { call(app, "PATCH", &path, Some(&cookie), None, Some(patch)).await }
        });
        tokio::time::timeout(hub.rpc_timeout(), reached)
            .await
            .unwrap()
            .unwrap();
        let mut revoke = f.backend.begin_write().await.unwrap();
        let crate::db::backend::OperationTx::SqliteFamily(writer) = revoke.operation() else {
            unreachable!()
        };
        writer.execute("DELETE FROM project_members WHERE workspace_id=?1 AND project_id=?2 AND user_id=?3",&[crate::db::codec::Cell::uuid(f.workspace),crate::db::codec::Cell::uuid(project),crate::db::codec::Cell::uuid(f.user)]).await.unwrap();
        revoke.commit().await.unwrap();
        proceed.send(()).unwrap();
        let (code, problem) = pending.await.unwrap();
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(problem["code"], "collab_unavailable");
        let after: (String, i64) =
            sqlx::query_as("SELECT content_json,version FROM tasks WHERE id=?1")
                .bind(task.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&after.0).unwrap(),
            before.content_json
        );
        assert_eq!(after.1, i64::from(before.meta.version));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM task_collab_updates WHERE workspace_id=?1 AND task_id=?2"
            )
            .bind(f.workspace.as_bytes().as_slice())
            .bind(task.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            history
        );
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'lead')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let (code, meta) = call(
            app.clone(),
            "PATCH",
            &path,
            Some(&cookie),
            None,
            Some(patch),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{meta}");
        assert_eq!(meta["id"], task.to_string());
        let healthy =
            crate::db::tasks::get_task_backend(&f.backend, f.workspace, task, f.user, credential)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(healthy.content_json["content"][0]["attrs"]["id"], "b-1");
        assert_eq!(
            healthy.content_json["content"][0]["content"][0]["text"],
            "不应发布 中 😀"
        );
        // Cancel a real route while its native projection is suspended. The
        // actor may finish the read; cancellation must not publish the PATCH.
        let live = hub.project_live(key, f.user, credential).await.unwrap();
        let (reached, proceed) = crate::collab::room::arm_native_consumer_barrier(
            task,
            crate::collab::room::NATIVE_PROJECT_FINAL_PROOF,
        )
        .await;
        let cancelled = tokio::spawn({
            let app = app.clone();
            let path = path.clone();
            let cookie = cookie.clone();
            async move {
                call(
                    app,
                    "PATCH",
                    &path,
                    Some(&cookie),
                    None,
                    Some(
                        json!({"type":"paragraph","content":[{"type":"text","text":"Cancelled"}]}),
                    ),
                )
                .await
            }
        });
        tokio::time::timeout(hub.rpc_timeout(), reached)
            .await
            .unwrap()
            .unwrap();
        cancelled.abort();
        assert!(cancelled.await.unwrap_err().is_cancelled());
        proceed.send(()).unwrap();
        let observed = hub.project_live(key, f.user, credential).await.unwrap();
        assert_eq!(observed.content_json, live.content_json);
        assert_eq!(observed.tail_seq, live.tail_seq);
        let (code, meta) = call(app,"PATCH",&path,Some(&cookie),None,Some(json!({"type":"paragraph","content":[{"type":"text","text":"Healthy after cancel 中 😀"}]}))).await;
        assert_eq!(code, StatusCode::OK, "{meta}");
        let next =
            crate::db::tasks::get_task_backend(&f.backend, f.workspace, task, f.user, credential)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(next.content_json["content"][0]["attrs"]["id"], "b-1");
        assert_eq!(
            next.content_json["content"][0]["content"][0]["text"],
            "Healthy after cancel 中 😀"
        );
        assert!(hub.shutdown().await.is_clean());
        drop(admission);
        f.close().await;
    }
}
