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
};
use crate::auth::scopes::{grants_api_token_scope, ApiTokenScope};
use crate::collab::derived_body::{prepare_derived_body, DerivedBodyError};
use crate::collab::room::{BodyWriteError, RoomKey};
use crate::collab::seed::{SeedEngine, SeedError};
use crate::db::revisions::{authorize_revision_target, RevisionDbError, RevisionTarget};
use crate::db::task_origins::{
    create_document_task, get_task_origin, origin_request_hash, DocumentTaskRequest,
    TaskOriginDbError, TASK_ORIGIN_ANCHOR_MAX_CHARS,
};
use crate::db::tasks::{get_task, CreateTaskInput};
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
        RevisionDbError::NotFound | RevisionDbError::Forbidden => {
            AppError::from_code(ProblemCode::NotFound).into()
        }
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

fn map_origin_error(err: TaskOriginDbError) -> TaskApiError {
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
fn normalized_task_input(body: &CreateTaskBody) -> Value {
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
    let request_hash = origin_request_hash(
        auth.user_id,
        input.project_id,
        input.anchor.as_deref(),
        &normalized_task_input(&task),
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
    let items: Vec<TaskOriginItemOutput> = page
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
        .collect();
    Ok(Json(TaskOriginListResponse {
        count: items.len(),
        items,
        next_cursor: page.next_cursor.map(|id| id.to_string()),
    }))
}
