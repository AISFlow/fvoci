use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::dto::{
    NotificationItemOutput, NotificationListResponse, NotificationPatchBody, NotificationPrefsBody,
    NotificationReadAllResponse, NotificationUnreadCountResponse, OkResponse,
};
use crate::auth::scopes::{grants_api_token_scope, ApiTokenScope};
use crate::db::notifications::{
    get_prefs, list_me_notifications, list_notifications, put_prefs, read_all, set_flags,
    unread_count, ContentKind, ListNotificationsQuery, NotificationFilter, NotificationPrefs,
    NotificationRow, SetNotificationFlags,
};
use crate::error::AppError;
use crate::http::authz::{require_request_auth, Access, RequestAuth};
use crate::http::guard::check_origin;
use crate::http::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationListQuery {
    pub filter: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i32>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/me/notifications", get(list_me_notifications_route))
        .route(
            "/api/v1/workspaces/{workspace_id}/notifications",
            get(list_workspace_notifications),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/notifications/unread-count",
            get(unread_count_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/notifications/read-all",
            post(read_all_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/notifications/{id}",
            axum::routing::patch(patch_notification),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/notification-prefs",
            get(get_prefs_route).put(put_prefs_route),
        )
}

fn item_output(row: NotificationRow) -> NotificationItemOutput {
    NotificationItemOutput {
        id: row.id,
        workspace_id: row.workspace_id,
        event_id: row.event_id,
        verb: row.verb,
        actor_user_id: row.actor_user_id,
        actor_given_name: row.actor_given_name,
        actor_family_name: row.actor_family_name,
        target_type: row.target_type,
        target_id: row.target_id,
        display_id: row.display_id,
        payload: row.payload,
        read_at: row.read_at,
        archived_at: row.archived_at,
        created_at: row.created_at,
    }
}

fn allowed_content_kinds(auth: &RequestAuth) -> Option<Vec<ContentKind>> {
    let scopes = auth.token_scopes.as_deref()?;
    let mut kinds = Vec::new();
    if grants_api_token_scope(scopes, ApiTokenScope::DocumentsRead) {
        kinds.push(ContentKind::Document);
    }
    if grants_api_token_scope(scopes, ApiTokenScope::TasksRead) {
        kinds.push(ContentKind::Task);
    }
    Some(kinds)
}

fn parse_list_query(
    query: NotificationListQuery,
) -> Result<(NotificationFilter, Option<String>, i32), AppError> {
    let filter = NotificationFilter::parse(query.filter.as_deref().unwrap_or("all"))
        .ok_or_else(|| AppError::from_code(crate::error::ProblemCode::InvalidInput))?;
    if query.cursor.as_ref().is_some_and(|c| c.len() > 1024) {
        return Err(crate::db::notifications::NotificationDbError::InvalidCursor.into());
    }
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(AppError::from_code(crate::error::ProblemCode::InvalidInput));
    }
    Ok((filter, query.cursor, limit))
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}

async fn list_workspace_notifications(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    query: Result<Query<NotificationListQuery>, QueryRejection>,
) -> Result<Json<NotificationListResponse>, AppError> {
    let Query(query) = query.map_err(AppError::from)?;
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let (filter, cursor, limit) = parse_list_query(query)?;
    let kinds = allowed_content_kinds(&auth);
    let page = list_notifications(
        &state.auth.db.pool,
        ListNotificationsQuery {
            workspace_id,
            user_id: auth.user_id,
            session_id: auth.credential_id,
            filter,
            cursor: cursor.as_deref(),
            limit,
            allowed_kinds: kinds.as_deref(),
        },
    )
    .await
    .map_err(internal)?;
    match page {
        Ok(page) => Ok(Json(NotificationListResponse {
            items: page.items.into_iter().map(item_output).collect(),
            next_cursor: page.next_cursor,
        })),
        Err(err) => Err(err.into()),
    }
}

async fn unread_count_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<NotificationUnreadCountResponse>, AppError> {
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let kinds = allowed_content_kinds(&auth);
    let count = unread_count(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        kinds.as_deref(),
    )
    .await
    .map_err(internal)?;
    match count {
        Ok(count) => Ok(Json(NotificationUnreadCountResponse { count })),
        Err(err) => Err(err.into()),
    }
}

async fn patch_notification(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, id)): Path<(Uuid, Uuid)>,
    body: Result<Json<NotificationPatchBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    if body.read.is_none() && body.archived.is_none() {
        return Err(AppError::from_code(crate::error::ProblemCode::InvalidInput));
    }
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let kinds = allowed_content_kinds(&auth);
    let ok = set_flags(
        &state.auth.db.pool,
        SetNotificationFlags {
            workspace_id,
            user_id: auth.user_id,
            session_id: auth.credential_id,
            notification_id: id,
            read: body.read,
            archived: body.archived,
            allowed_kinds: kinds.as_deref(),
        },
    )
    .await
    .map_err(internal)?;
    match ok {
        Ok(true) => Ok(Json(OkResponse { ok: true })),
        Ok(false) => Err(AppError::from_code(crate::error::ProblemCode::NotFound)),
        Err(err) => Err(err.into()),
    }
}

async fn read_all_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<NotificationReadAllResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let kinds = allowed_content_kinds(&auth);
    let updated = read_all(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        kinds.as_deref(),
    )
    .await
    .map_err(internal)?;
    match updated {
        Ok(updated) => Ok(Json(NotificationReadAllResponse { updated })),
        Err(err) => Err(err.into()),
    }
}

async fn get_prefs_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<NotificationPrefsBody>, AppError> {
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let prefs = get_prefs(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
    )
    .await
    .map_err(internal)?;
    match prefs {
        Ok(prefs) => Ok(Json(prefs_body(prefs))),
        Err(err) => Err(err.into()),
    }
}

async fn put_prefs_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<NotificationPrefsBody>, JsonRejection>,
) -> Result<Json<NotificationPrefsBody>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let prefs = put_prefs(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        NotificationPrefs {
            in_app: body.in_app,
            mail_immediate: body.mail_immediate,
            mail_digest: body.mail_digest,
        },
    )
    .await
    .map_err(internal)?;
    match prefs {
        Ok(prefs) => Ok(Json(prefs_body(prefs))),
        Err(err) => Err(err.into()),
    }
}

fn prefs_body(prefs: NotificationPrefs) -> NotificationPrefsBody {
    NotificationPrefsBody {
        in_app: prefs.in_app,
        mail_immediate: prefs.mail_immediate,
        mail_digest: prefs.mail_digest,
    }
}

async fn list_me_notifications_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    query: Result<Query<NotificationListQuery>, QueryRejection>,
) -> Result<Json<NotificationListResponse>, AppError> {
    let Query(query) = query.map_err(AppError::from)?;
    let auth = require_request_auth(&state, &headers, &jar, Access::Session, None).await?;
    let (filter, cursor, limit) = parse_list_query(query)?;
    let page = list_me_notifications(
        &state.auth.db.pool,
        auth.user_id,
        auth.credential_id,
        filter,
        cursor.as_deref(),
        limit,
    )
    .await
    .map_err(internal)?;
    match page {
        Ok(page) => Ok(Json(NotificationListResponse {
            items: page.items.into_iter().map(item_output).collect(),
            next_cursor: page.next_cursor,
        })),
        Err(err) => Err(err.into()),
    }
}
