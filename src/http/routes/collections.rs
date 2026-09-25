//! Source `apps/server/src/domains/documents/collections.ts`: collections,
//! fields, items, values, queries and collection views. Scope `documents.*`
//! for API tokens (the source registers these under the documents domain).

use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::api::collections_dto::{
    CollectionFieldListResponse, CollectionFieldOutput, CollectionItemLookupResponse,
    CollectionItemOutput, CollectionListResponse, CollectionOptionOutput, CollectionOutput,
    CollectionQueryDayOutput, CollectionQueryGroupOutput, CollectionQueryItemOutput,
    CollectionQueryPreviewOutput, CollectionQueryResponse, CollectionValueResponse,
    CollectionViewListResponse, CollectionViewOutput, ProjectCollectionOutput,
};
use crate::api::dto::OkResponse;
use crate::auth::scopes::ApiTokenScope;
use crate::collections::{
    iso_millis, parse_attach, parse_collection_create, parse_collection_view, parse_field_create,
    parse_field_patch, parse_query_input, parse_value_input, AttachTarget,
};
use crate::db::collection_query::{query_collection, QueryRow};
use crate::db::collections::{
    attach_item, create_collection, create_field, item_for_target, list_collections, list_fields,
    list_views, patch_field, project_collection, put_value, remove_view, save_view, Actor,
    CollectionDbError, CollectionRow, CollectionViewRow, FieldRow, ItemRow,
};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access};
use crate::http::guard::check_origin;
use crate::http::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/collections",
            get(list_route).post(create_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/fields",
            get(fields_route).post(create_field_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/fields/{field_id}",
            patch(patch_field_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/items",
            post(attach_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/items/{item_id}/values",
            put(put_value_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/query",
            post(query_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/views",
            get(views_route).post(create_view_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/views/{view_id}",
            patch(update_view_route).delete(remove_view_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/collection-item",
            get(document_item_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/collection-item",
            get(task_item_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/collection",
            get(project_collection_route),
        )
}

/// Problem responses; version/archival conflicts keep the source codes.
#[derive(Debug)]
pub(crate) enum ApiError {
    App(AppError),
    Coded {
        status: StatusCode,
        code: &'static str,
        title: &'static str,
    },
}

impl From<AppError> for ApiError {
    fn from(value: AppError) -> Self {
        Self::App(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::App(err) => err.into_response(),
            Self::Coded {
                status,
                code,
                title,
            } => {
                let mut headers = HeaderMap::new();
                headers.insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/problem+json"),
                );
                let body = json!({
                    "type": "about:blank",
                    "title": title,
                    "status": status.as_u16(),
                    "code": code,
                });
                (status, headers, Json(body)).into_response()
            }
        }
    }
}

pub(crate) fn invalid() -> ApiError {
    AppError::from_code(ProblemCode::InvalidInput).into()
}

pub(crate) fn version_conflict() -> ApiError {
    ApiError::Coded {
        status: StatusCode::CONFLICT,
        code: "document_version_mismatch",
        title: "document version mismatch",
    }
}

pub(crate) fn internal(err: sqlx::Error) -> ApiError {
    tracing::error!("database error: {}", err);
    AppError::internal().into()
}

pub(crate) fn map_error(err: CollectionDbError) -> ApiError {
    match err {
        CollectionDbError::NotFound => AppError::from_code(ProblemCode::NotFound).into(),
        CollectionDbError::Forbidden => {
            AppError::from_code(ProblemCode::InsufficientPermissions).into()
        }
        CollectionDbError::InvalidInput => invalid(),
        CollectionDbError::VersionConflict => version_conflict(),
        CollectionDbError::ProjectArchived => {
            AppError::from_code(ProblemCode::ProjectArchived).into()
        }
        CollectionDbError::TaskArchived => ApiError::Coded {
            status: StatusCode::CONFLICT,
            code: "task_archived",
            title: "task archived",
        },
        CollectionDbError::InvalidCursor => AppError {
            status: StatusCode::BAD_REQUEST,
            code: ProblemCode::InvalidInput,
            source: None,
            params: Some(json!({"code": "invalid_cursor"})),
            retry_after: None,
        }
        .into(),
    }
}

pub(crate) async fn actor(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    scope: ApiTokenScope,
    workspace_id: Uuid,
    peer: Option<SocketAddr>,
) -> Result<Actor, ApiError> {
    let auth = require_request_auth(
        state,
        headers,
        jar,
        Access::Scope(scope),
        Some(workspace_id),
    )
    .await?;
    Ok(Actor {
        user_id: auth.user_id,
        credential_id: auth.credential_id,
        client_ip: peer.map(|peer| peer.ip()),
    })
}

pub(crate) fn json_body(body: Result<Json<Value>, JsonRejection>) -> Result<Value, ApiError> {
    body.map(|Json(value)| value)
        .map_err(|rejection| AppError::from(rejection).into())
}

fn collection_output(row: CollectionRow) -> CollectionOutput {
    CollectionOutput {
        id: row.id.to_string(),
        workspace_id: row.workspace_id.to_string(),
        project_id: row.project_id.map(|id| id.to_string()),
        kind: row.kind.as_str().to_string(),
        name: row.name,
        version: row.version,
        deleted_at: row.deleted_at.map(iso_millis),
    }
}

fn field_output(row: FieldRow) -> CollectionFieldOutput {
    CollectionFieldOutput {
        id: row.id.to_string(),
        collection_id: row.collection_id.to_string(),
        key: row.key,
        name: row.name,
        description: row.description,
        r#type: row.field_type.as_str().to_string(),
        version: row.version,
        sort_key: row.sort_key,
        deleted_at: row.deleted_at.map(iso_millis),
        options: row
            .options
            .into_iter()
            .map(|option| CollectionOptionOutput {
                id: option.id.to_string(),
                key: option.key,
                label: option.label,
                sort_key: option.sort_key,
                deleted_at: option.deleted_at.map(iso_millis),
            })
            .collect(),
    }
}

fn item_output(row: ItemRow) -> CollectionItemOutput {
    CollectionItemOutput {
        id: row.id.to_string(),
        collection_id: row.collection_id.to_string(),
        document_id: row.document_id.map(|id| id.to_string()),
        task_id: row.task_id.map(|id| id.to_string()),
        version: row.version,
    }
}

pub(crate) fn view_output(row: CollectionViewRow) -> CollectionViewOutput {
    CollectionViewOutput {
        id: row.id.to_string(),
        collection_id: row.collection_id.to_string(),
        owner_id: row.owner_id.to_string(),
        version: row.version,
        name: row.name,
        r#type: row.view_type,
        visibility: row.visibility,
        config: row.config,
    }
}

fn values_object(values: Vec<(Uuid, Value)>) -> Value {
    let mut map = Map::new();
    for (field_id, value) in values {
        map.insert(field_id.to_string(), value);
    }
    Value::Object(map)
}

fn query_item(row: QueryRow) -> CollectionQueryItemOutput {
    CollectionQueryItemOutput {
        id: row.id.to_string(),
        can_edit: row.can_edit,
        document_id: row.document_id.map(|id| id.to_string()),
        task_id: row.task_id.map(|id| id.to_string()),
        display_id: row.display_id,
        title: row.title,
        task_type: row.task_type,
        status_id: row.status_id.map(|id| id.to_string()),
        start_date: row.start_date.map(|d| d.to_string()),
        due_date: row.due_date.map(|d| d.to_string()),
        due_at: row.due_at.map(iso_millis),
        version: row.version,
        group: row.group,
        date: row.date.map(|d| d.to_string()),
        values: values_object(row.values),
    }
}

fn query_preview(row: QueryRow) -> CollectionQueryPreviewOutput {
    CollectionQueryPreviewOutput {
        id: row.id.to_string(),
        document_id: row.document_id.map(|id| id.to_string()),
        task_id: row.task_id.map(|id| id.to_string()),
        display_id: row.display_id,
        title: row.title,
        status_id: row.status_id.map(|id| id.to_string()),
        date: row.date.map(|d| d.to_string()),
        can_edit: row.can_edit,
        version: row.version,
        start_date: row.start_date.map(|d| d.to_string()),
        due_date: row.due_date.map(|d| d.to_string()),
        due_at: row.due_at.map(iso_millis),
        values: values_object(row.values),
    }
}

async fn list_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<CollectionListResponse>, ApiError> {
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
        None,
    )
    .await?;
    let items = list_collections(&state.auth.db.pool, workspace_id, &actor)
        .await
        .map_err(internal)?
        .map_err(map_error)?;
    Ok(Json(CollectionListResponse {
        items: items.into_iter().map(collection_output).collect(),
    }))
}

async fn create_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
        Some(peer),
    )
    .await?;
    let input = parse_collection_create(&json_body(body)?).map_err(|_| invalid())?;
    let row = create_collection(&state.auth.db.pool, workspace_id, &actor, &input)
        .await
        .map_err(internal)?
        .map_err(map_error)?;
    Ok((StatusCode::CREATED, Json(collection_output(row))).into_response())
}

async fn fields_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, collection_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<CollectionFieldListResponse>, ApiError> {
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
        None,
    )
    .await?;
    let fields = list_fields(&state.auth.db.pool, workspace_id, &actor, collection_id)
        .await
        .map_err(internal)?
        .map_err(map_error)?;
    Ok(Json(CollectionFieldListResponse {
        items: fields.into_iter().map(field_output).collect(),
    }))
}

async fn create_field_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, collection_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
        Some(peer),
    )
    .await?;
    let input = parse_field_create(&json_body(body)?).map_err(|_| invalid())?;
    let field = create_field(
        &state.auth.db.pool,
        workspace_id,
        &actor,
        collection_id,
        &input,
    )
    .await
    .map_err(internal)?
    .map_err(map_error)?;
    Ok((StatusCode::CREATED, Json(field_output(field))).into_response())
}

async fn patch_field_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, collection_id, field_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<CollectionFieldOutput>, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
        Some(peer),
    )
    .await?;
    let input = parse_field_patch(&json_body(body)?).map_err(|_| invalid())?;
    let field = patch_field(
        &state.auth.db.pool,
        workspace_id,
        &actor,
        collection_id,
        field_id,
        &input,
    )
    .await
    .map_err(internal)?
    .map_err(map_error)?;
    Ok(Json(field_output(field)))
}

async fn attach_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, collection_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
        None,
    )
    .await?;
    let target = parse_attach(&json_body(body)?).map_err(|_| invalid())?;
    let item = attach_item(
        &state.auth.db.pool,
        workspace_id,
        &actor,
        collection_id,
        target,
    )
    .await
    .map_err(internal)?
    .map_err(map_error)?;
    Ok((StatusCode::CREATED, Json(item_output(item))).into_response())
}

async fn put_value_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, collection_id, item_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<CollectionValueResponse>, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
        None,
    )
    .await?;
    let input = parse_value_input(&json_body(body)?).map_err(|_| invalid())?;
    let version = put_value(
        &state.auth.db.pool,
        workspace_id,
        &actor,
        collection_id,
        item_id,
        &input,
    )
    .await
    .map_err(internal)?
    .map_err(map_error)?;
    Ok(Json(CollectionValueResponse { version }))
}

async fn query_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, collection_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<CollectionQueryResponse>, ApiError> {
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
        None,
    )
    .await?;
    let input = parse_query_input(&json_body(body)?).map_err(|_| invalid())?;
    let result = query_collection(
        &state.auth.db.pool,
        workspace_id,
        &actor,
        collection_id,
        &input,
    )
    .await
    .map_err(internal)?
    .map_err(map_error)?;
    Ok(Json(CollectionQueryResponse {
        can_edit: result.can_edit,
        days: result
            .days
            .into_iter()
            .map(|(date, count)| CollectionQueryDayOutput {
                date: date.map(|d| d.to_string()),
                count,
            })
            .collect(),
        items: result.items.into_iter().map(query_item).collect(),
        groups: result
            .groups
            .into_iter()
            .map(|group| CollectionQueryGroupOutput {
                id: group.id,
                name: group.name,
                item_ids: group.item_ids.iter().map(Uuid::to_string).collect(),
                count: group.count,
                deleted: group.deleted,
            })
            .collect(),
        count: result.count,
        next_cursor: result.next_cursor,
        previews: result.previews.into_iter().map(query_preview).collect(),
    }))
}

async fn views_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, collection_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<CollectionViewListResponse>, ApiError> {
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
        None,
    )
    .await?;
    let list = list_views(&state.auth.db.pool, workspace_id, &actor, collection_id)
        .await
        .map_err(internal)?
        .map_err(map_error)?;
    Ok(Json(CollectionViewListResponse {
        can_save: list.can_save,
        can_manage: list.can_manage,
        items: list.items.into_iter().map(view_output).collect(),
    }))
}

async fn create_view_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, collection_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
        Some(peer),
    )
    .await?;
    let input = parse_collection_view(&json_body(body)?, false).map_err(|_| invalid())?;
    let view = save_view(
        &state.auth.db.pool,
        workspace_id,
        &actor,
        collection_id,
        None,
        &input,
    )
    .await
    .map_err(internal)?
    .map_err(map_error)?;
    Ok((StatusCode::CREATED, Json(view_output(view))).into_response())
}

async fn update_view_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, collection_id, view_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<CollectionViewOutput>, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
        Some(peer),
    )
    .await?;
    let input = parse_collection_view(&json_body(body)?, true).map_err(|_| invalid())?;
    let view = save_view(
        &state.auth.db.pool,
        workspace_id,
        &actor,
        collection_id,
        Some(view_id),
        &input,
    )
    .await
    .map_err(internal)?
    .map_err(map_error)?;
    Ok(Json(view_output(view)))
}

async fn remove_view_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, collection_id, view_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<OkResponse>, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
        Some(peer),
    )
    .await?;
    remove_view(
        &state.auth.db.pool,
        workspace_id,
        &actor,
        collection_id,
        view_id,
    )
    .await
    .map_err(internal)?
    .map_err(map_error)?;
    Ok(Json(OkResponse { ok: true }))
}

async fn item_lookup(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    target: AttachTarget,
) -> Result<Json<CollectionItemLookupResponse>, ApiError> {
    let actor = actor(
        state,
        headers,
        jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
        None,
    )
    .await?;
    let lookup = item_for_target(&state.auth.db.pool, workspace_id, &actor, target)
        .await
        .map_err(internal)?
        .map_err(map_error)?;
    Ok(Json(CollectionItemLookupResponse {
        item: lookup.item.map(item_output),
        values: values_object(lookup.values),
        can_edit: lookup.can_edit,
    }))
}

async fn document_item_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<CollectionItemLookupResponse>, ApiError> {
    item_lookup(
        &state,
        &headers,
        &jar,
        workspace_id,
        AttachTarget::Document(document_id),
    )
    .await
}

async fn task_item_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<CollectionItemLookupResponse>, ApiError> {
    item_lookup(
        &state,
        &headers,
        &jar,
        workspace_id,
        AttachTarget::Task(task_id),
    )
    .await
}

async fn project_collection_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ProjectCollectionOutput>, ApiError> {
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
        None,
    )
    .await?;
    let found = project_collection(&state.auth.db.pool, workspace_id, &actor, project_id)
        .await
        .map_err(internal)?
        .map_err(map_error)?;
    let base = collection_output(found.collection);
    Ok(Json(ProjectCollectionOutput {
        id: base.id,
        workspace_id: base.workspace_id,
        project_id: base.project_id,
        kind: base.kind,
        name: base.name,
        version: base.version,
        deleted_at: base.deleted_at,
        can_edit: found.can_edit,
        can_manage: found.can_manage,
    }))
}
