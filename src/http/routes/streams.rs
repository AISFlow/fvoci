use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use axum_extra::extract::CookieJar;
use futures_util::Stream;
use serde_json::json;
use uuid::Uuid;

use crate::db::context::{session_is_live, set_tenant};
use crate::db::projects::{lock_project, project_permission};
use crate::db::workspace::membership_role;
use crate::error::{AppError, ProblemCode};
use crate::http::guard::check_origin;
use crate::http::routes::tasks::{internal, require_session};
use crate::http::state::AppState;
use crate::projects::ProjectPermission;
use crate::streams::{
    access_event_targets_user, initial_cursor, poll_access_events, poll_task_events, EventCursor,
    StreamAcquireError, StreamGuard, StreamHub, STREAM_CHANNEL_CAPACITY, STREAM_KEEPALIVE,
    STREAM_POLL_INTERVAL,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/stream",
            get(project_task_stream),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/access-stream",
            get(workspace_access_stream),
        )
}

async fn project_task_stream(
    State(state): State<AppState>,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let guard = match state.streams.try_acquire() {
        Ok(guard) => guard,
        Err(StreamAcquireError::Capacity) => {
            return Err(AppError::from_code(ProblemCode::RateLimitExceeded));
        }
        Err(StreamAcquireError::Stopped) => return Ok(stream_stopped()),
    };
    let (_, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    admit_project_view(&state, workspace_id, project_id, user_id, session_id).await?;

    let pool = state.auth.db.pool.clone();
    let hub = state.streams.clone();
    let cursor = initial_cursor(&pool, workspace_id)
        .await
        .map_err(internal)?;
    let stream = task_sse_stream(
        hub,
        pool,
        workspace_id,
        project_id,
        user_id,
        session_id,
        cursor,
        guard,
    );
    Ok(sse_response(stream))
}

async fn workspace_access_stream(
    State(state): State<AppState>,
    Path(workspace_id): Path<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let guard = match state.streams.try_acquire() {
        Ok(guard) => guard,
        Err(StreamAcquireError::Capacity) => {
            return Err(AppError::from_code(ProblemCode::RateLimitExceeded));
        }
        Err(StreamAcquireError::Stopped) => return Ok(stream_stopped()),
    };
    let (_, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        Some(workspace_id),
    )
    .await?;
    admit_workspace_member(&state, workspace_id, user_id, session_id).await?;

    let pool = state.auth.db.pool.clone();
    let hub = state.streams.clone();
    let cursor = initial_cursor(&pool, workspace_id)
        .await
        .map_err(internal)?;
    let stream = access_sse_stream(hub, pool, workspace_id, user_id, session_id, cursor, guard);
    Ok(sse_response(stream))
}

async fn admit_project_view(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<(), AppError> {
    let pool = &state.auth.db.pool;
    let mut tx = pool.begin().await.map_err(internal)?;
    set_tenant(&mut tx, workspace_id).await.map_err(internal)?;
    if !session_is_live(&mut tx, user_id, session_id).await.map_err(internal)? {
        tx.rollback().await.ok();
        return Err(AppError::from_code(ProblemCode::AuthenticationRequired));
    }
    let Some(locked) = lock_project(&mut tx, workspace_id, project_id)
        .await
        .map_err(internal)?
    else {
        tx.rollback().await.ok();
        return Err(AppError::from_code(ProblemCode::NotFound));
    };
    let permission = project_permission(&mut tx, workspace_id, user_id, &locked)
        .await
        .map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    if !permission.at_least(ProjectPermission::View) {
        return Err(AppError::from_code(ProblemCode::NotFound));
    }
    Ok(())
}

async fn admit_workspace_member(
    state: &AppState,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<(), AppError> {
    let pool = &state.auth.db.pool;
    let mut tx = pool.begin().await.map_err(internal)?;
    set_tenant(&mut tx, workspace_id).await.map_err(internal)?;
    if !session_is_live(&mut tx, user_id, session_id).await.map_err(internal)? {
        tx.rollback().await.ok();
        return Err(AppError::from_code(ProblemCode::AuthenticationRequired));
    }
    let role = membership_role(&mut tx, workspace_id, user_id)
        .await
        .map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    if role.is_none() {
        return Err(AppError::from_code(ProblemCode::NotFound));
    }
    Ok(())
}

fn stream_stopped() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(json!({
            "type": "about:blank",
            "title": "stream stopped",
            "status": 503,
            "code": "stream_stopped",
        })),
    )
        .into_response()
}

fn sse_response<S>(stream: S) -> Response
where
    S: Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(STREAM_KEEPALIVE)
                .text("keep-alive"),
        )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache, no-transform"),
    );
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

fn task_sse_stream(
    hub: Arc<StreamHub>,
    pool: sqlx::PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    cursor: EventCursor,
    guard: StreamGuard,
) -> EventReceiverStream {
    let (tx, rx) = tokio::sync::mpsc::channel(STREAM_CHANNEL_CAPACITY);
    tokio::spawn(async move {
        let _guard = guard;
        if send_event(&tx, Event::default().event("open").data("{}")).is_err() {
            return;
        }
        let mut cursor = cursor;
        loop {
            if tx.is_closed() {
                return;
            }
            if !hub.accepting() {
                return;
            }
            if !session_still_valid(&pool, workspace_id, user_id, session_id).await {
                return;
            }
            if !project_still_viewable(&pool, workspace_id, project_id, user_id).await {
                return;
            }
            tokio::select! {
                _ = tx.closed() => return,
                _ = tokio::time::sleep(STREAM_POLL_INTERVAL) => {}
            }
            match poll_task_events(&pool, workspace_id, project_id, &cursor, 50).await {
                Ok(rows) => {
                    for row in rows {
                        let task_id = row
                            .payload
                            .get("taskId")
                            .and_then(|v| v.as_str())
                            .filter(|id| !id.is_empty());
                        let Some(task_id) = task_id else {
                            cursor = EventCursor {
                                xact: row.xact.clone(),
                                seq: row.seq,
                            };
                            continue;
                        };
                        let data = json!({"verb": row.verb, "taskId": task_id}).to_string();
                        let event = Event::default().event("task").data(data);
                        if send_event(&tx, event).is_err() {
                            return;
                        }
                        cursor = EventCursor {
                            xact: row.xact.clone(),
                            seq: row.seq,
                        };
                    }
                }
                Err(err) => {
                    tracing::warn!("task stream poll failed: {}", err);
                }
            }
        }
    });
    EventReceiverStream { rx }
}

fn access_sse_stream(
    hub: Arc<StreamHub>,
    pool: sqlx::PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    mut cursor: EventCursor,
    guard: StreamGuard,
) -> EventReceiverStream {
    let (tx, rx) = tokio::sync::mpsc::channel(STREAM_CHANNEL_CAPACITY);
    tokio::spawn(async move {
        let _guard = guard;
        let _keepalive_sender = tx;
        loop {
            if _keepalive_sender.is_closed() {
                return;
            }
            if !hub.accepting() {
                return;
            }
            if !session_still_valid(&pool, workspace_id, user_id, session_id).await {
                return;
            }
            if !membership_role_only(&pool, workspace_id, user_id).await {
                return;
            }
            tokio::select! {
                _ = _keepalive_sender.closed() => return,
                _ = tokio::time::sleep(STREAM_POLL_INTERVAL) => {}
            }
            match poll_access_events(&pool, workspace_id, user_id, &cursor, 50).await {
                Ok(rows) => {
                    for row in rows {
                        cursor = EventCursor {
                            xact: row.xact.clone(),
                            seq: row.seq,
                        };
                        if access_event_targets_user(&row, user_id) {
                            return;
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!("access stream poll failed: {}", err);
                }
            }
        }
    });
    EventReceiverStream { rx }
}

struct EventReceiverStream {
    rx: tokio::sync::mpsc::Receiver<Result<Event, Infallible>>,
}

impl Stream for EventReceiverStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().rx.poll_recv(cx)
    }
}

fn send_event(
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
    event: Event,
) -> Result<(), ()> {
    match tx.try_send(Ok(event)) {
        Ok(()) => Ok(()),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Err(()),
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => Err(()),
    }
}

async fn session_still_valid(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
) -> bool {
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(_) => return false,
    };
    if set_tenant(&mut tx, workspace_id).await.is_err() {
        return false;
    }
    let live = session_is_live(&mut tx, user_id, session_id)
        .await
        .unwrap_or(false);
    let _ = tx.commit().await;
    live
}

async fn project_still_viewable(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
) -> bool {
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(_) => return false,
    };
    if set_tenant(&mut tx, workspace_id).await.is_err() {
        return false;
    }
    let Some(locked) = lock_project(&mut tx, workspace_id, project_id)
        .await
        .ok()
        .flatten()
    else {
        return false;
    };
    let ok = project_permission(&mut tx, workspace_id, user_id, &locked)
        .await
        .ok()
        .is_some_and(|p| p.at_least(ProjectPermission::View));
    let _ = tx.commit().await;
    ok
}

async fn membership_role_only(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> bool {
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(_) => return false,
    };
    if set_tenant(&mut tx, workspace_id).await.is_err() {
        return false;
    }
    let role = membership_role(&mut tx, workspace_id, user_id).await.ok().flatten();
    let _ = tx.commit().await;
    role.is_some()
}
