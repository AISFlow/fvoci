use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

#[cfg(feature = "db-tests")]
use std::collections::BTreeMap;
#[cfg(feature = "db-tests")]
use std::sync::Mutex;

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

use crate::error::{AppError, ProblemCode};
use crate::http::guard::check_origin;
use crate::http::routes::tasks::{internal, require_session};
use crate::http::state::AppState;
use crate::streams::{
    initial_cursor, poll_access_events, poll_task_events, project_stream_access,
    task_stream_wire_hint, workspace_stream_access, EventCursor, StreamAccess, StreamAcquireError,
    StreamGuard, StreamHub, STREAM_CHANNEL_CAPACITY, STREAM_HIGH_WATER_MARK, STREAM_KEEPALIVE,
    STREAM_POLL_INTERVAL,
};

#[cfg(feature = "db-tests")]
static TASK_HINT_ENQUEUED_BY_PROJECT: Mutex<BTreeMap<(Uuid, Uuid), usize>> =
    Mutex::new(BTreeMap::new());

/// Integration-test barrier: producer successfully queued a task hint (not delivered).
pub fn task_stream_task_hint_enqueue_count(workspace_id: Uuid, project_id: Uuid) -> usize {
    #[cfg(feature = "db-tests")]
    {
        return TASK_HINT_ENQUEUED_BY_PROJECT
            .lock()
            .expect("task hint witness")
            .get(&(workspace_id, project_id))
            .copied()
            .unwrap_or(0);
    }
    #[cfg(not(feature = "db-tests"))]
    {
        let _ = (workspace_id, project_id);
        0
    }
}

pub fn reset_task_stream_task_hint_enqueue_count(workspace_id: Uuid, project_id: Uuid) {
    #[cfg(feature = "db-tests")]
    {
        TASK_HINT_ENQUEUED_BY_PROJECT
            .lock()
            .expect("task hint witness")
            .remove(&(workspace_id, project_id));
    }
    #[cfg(not(feature = "db-tests"))]
    {
        let _ = (workspace_id, project_id);
    }
}

#[cfg(feature = "db-tests")]
fn record_task_hint_enqueued(workspace_id: Uuid, project_id: Uuid) {
    let mut map = TASK_HINT_ENQUEUED_BY_PROJECT
        .lock()
        .expect("task hint witness");
    let key = (workspace_id, project_id);
    let next = map.get(&key).copied().unwrap_or(0) + 1;
    map.insert(key, next);
}

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
    admit(
        project_stream_access(
            &state.auth.db.pool,
            workspace_id,
            project_id,
            user_id,
            session_id,
        )
        .await,
    )?;

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
    admit(workspace_stream_access(&state.auth.db.pool, workspace_id, user_id, session_id).await)?;

    let pool = state.auth.db.pool.clone();
    let hub = state.streams.clone();
    let cursor = initial_cursor(&pool, workspace_id)
        .await
        .map_err(internal)?;
    let stream = access_sse_stream(hub, pool, workspace_id, user_id, session_id, cursor, guard);
    Ok(sse_response(stream))
}

/// Admission keeps the 401 (credential gone) / 404 (no access) distinction.
fn admit(access: Result<StreamAccess, sqlx::Error>) -> Result<(), AppError> {
    match access.map_err(internal)? {
        StreamAccess::Allowed => Ok(()),
        StreamAccess::CredentialDead => {
            Err(AppError::from_code(ProblemCode::AuthenticationRequired))
        }
        StreamAccess::Denied => Err(AppError::from_code(ProblemCode::NotFound)),
    }
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

#[derive(Clone)]
enum TaskStreamQueueItem {
    Open,
    TaskHint { wire_verb: String, task_id: String },
}

#[derive(Clone)]
struct TaskStreamAuth {
    pool: sqlx::PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
}

/// Single bounded hint queue; HTTP `Stream` polls hints and authorizes on consumption.
struct TaskAuthorizedSseStream {
    queue_rx: tokio::sync::mpsc::Receiver<TaskStreamQueueItem>,
    queue_body_bytes: Arc<AtomicUsize>,
    auth: TaskStreamAuth,
    pending: Option<TaskStreamQueueItem>,
    authorize: Option<Pin<Box<dyn Future<Output = bool> + Send>>>,
    _guard: StreamGuard,
}

impl Stream for TaskAuthorizedSseStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.pending.is_none() {
            match this.queue_rx.poll_recv(cx) {
                Poll::Ready(Some(item)) => this.pending = Some(item),
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }

        if this.authorize.is_none() {
            let item = this.pending.as_ref().expect("pending with authorize");
            this.authorize = Some(authorize_queue_item(&this.auth, item));
        }

        let authorize = this.authorize.as_mut().expect("authorize future");
        match authorize.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(false) => {
                drain_task_queue(&mut this.queue_rx, &this.queue_body_bytes);
                this.pending = None;
                this.authorize = None;
                Poll::Ready(None)
            }
            Poll::Ready(true) => {
                let item = this.pending.take().expect("pending after auth");
                this.authorize = None;
                release_queue_body_bytes(&this.queue_body_bytes, task_item_body_bytes(&item));
                Poll::Ready(Some(Ok(queue_item_to_event(item))))
            }
        }
    }
}

fn authorize_queue_item(
    auth: &TaskStreamAuth,
    item: &TaskStreamQueueItem,
) -> Pin<Box<dyn Future<Output = bool> + Send>> {
    let pool = auth.pool.clone();
    let workspace_id = auth.workspace_id;
    let project_id = auth.project_id;
    let user_id = auth.user_id;
    let session_id = auth.session_id;
    match item {
        TaskStreamQueueItem::Open | TaskStreamQueueItem::TaskHint { .. } => Box::pin(async move {
            // Fail closed: a DB error withholds the item like a denial.
            matches!(
                project_stream_access(&pool, workspace_id, project_id, user_id, session_id).await,
                Ok(StreamAccess::Allowed)
            )
        }),
    }
}

fn queue_item_to_event(item: TaskStreamQueueItem) -> Event {
    match item {
        TaskStreamQueueItem::Open => Event::default().event("open").data("{}"),
        TaskStreamQueueItem::TaskHint { wire_verb, task_id } => {
            let data = json!({"verb": wire_verb, "taskId": task_id}).to_string();
            Event::default().event("task").data(data)
        }
    }
}

fn task_item_body_bytes(item: &TaskStreamQueueItem) -> usize {
    match item {
        TaskStreamQueueItem::Open => "event: open\ndata: {}\n\n".len(),
        TaskStreamQueueItem::TaskHint { wire_verb, task_id } => {
            let data = json!({"verb": wire_verb, "taskId": task_id}).to_string();
            format!("event: task\ndata: {data}\n\n").len()
        }
    }
}

fn reserve_queue_body_bytes(budget: &AtomicUsize, bytes: usize, limit: usize) -> bool {
    loop {
        let current = budget.load(Ordering::Acquire);
        let projected = current + bytes;
        if projected > limit {
            return false;
        }
        if budget
            .compare_exchange_weak(current, projected, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return true;
        }
    }
}

fn release_queue_body_bytes(budget: &AtomicUsize, bytes: usize) {
    budget
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            Some(current.saturating_sub(bytes))
        })
        .expect("queue byte budget");
}

fn try_enqueue_task_hint(
    queue_tx: &tokio::sync::mpsc::Sender<TaskStreamQueueItem>,
    queue_body_bytes: &Arc<AtomicUsize>,
    workspace_id: Uuid,
    project_id: Uuid,
    item: TaskStreamQueueItem,
) -> Result<(), ()> {
    let bytes = task_item_body_bytes(&item);
    if !reserve_queue_body_bytes(queue_body_bytes, bytes, STREAM_HIGH_WATER_MARK) {
        return Err(());
    }
    let is_hint = matches!(&item, TaskStreamQueueItem::TaskHint { .. });
    if queue_tx.try_send(item).is_err() {
        release_queue_body_bytes(queue_body_bytes, bytes);
        return Err(());
    }
    if is_hint {
        record_task_hint_enqueued(workspace_id, project_id);
    }
    Ok(())
}

#[cfg(not(feature = "db-tests"))]
fn record_task_hint_enqueued(_workspace_id: Uuid, _project_id: Uuid) {}

fn drain_task_queue(
    queue_rx: &mut tokio::sync::mpsc::Receiver<TaskStreamQueueItem>,
    queue_body_bytes: &Arc<AtomicUsize>,
) {
    while let Ok(item) = queue_rx.try_recv() {
        release_queue_body_bytes(queue_body_bytes, task_item_body_bytes(&item));
    }
}

#[allow(clippy::too_many_arguments)]
fn task_sse_stream(
    hub: Arc<StreamHub>,
    pool: sqlx::PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    cursor: EventCursor,
    guard: StreamGuard,
) -> TaskAuthorizedSseStream {
    let (queue_tx, queue_rx) = tokio::sync::mpsc::channel(STREAM_CHANNEL_CAPACITY);
    let queue_body_bytes = Arc::new(AtomicUsize::new(0));
    let auth = TaskStreamAuth {
        pool: pool.clone(),
        workspace_id,
        project_id,
        user_id,
        session_id,
    };

    let queue_body_bytes_producer = Arc::clone(&queue_body_bytes);
    tokio::spawn(async move {
        if try_enqueue_task_hint(
            &queue_tx,
            &queue_body_bytes_producer,
            workspace_id,
            project_id,
            TaskStreamQueueItem::Open,
        )
        .is_err()
        {
            return;
        }
        let mut cursor = cursor;
        loop {
            if queue_tx.is_closed() {
                return;
            }
            if !hub.accepting() {
                return;
            }
            tokio::select! {
                _ = queue_tx.closed() => return,
                _ = tokio::time::sleep(STREAM_POLL_INTERVAL) => {}
            }
            // The credential check shares the poll transaction, so a revoked
            // session or token ends the stream within one tick.
            match poll_task_events(
                &pool,
                workspace_id,
                project_id,
                user_id,
                session_id,
                &cursor,
                50,
            )
            .await
            {
                Ok(None) => return,
                Ok(Some(rows)) => {
                    for row in rows {
                        let Some((wire_verb, task_id)) = task_stream_wire_hint(&row) else {
                            cursor = EventCursor {
                                xact: row.xact.clone(),
                                seq: row.seq,
                            };
                            continue;
                        };
                        if try_enqueue_task_hint(
                            &queue_tx,
                            &queue_body_bytes_producer,
                            workspace_id,
                            project_id,
                            TaskStreamQueueItem::TaskHint { wire_verb, task_id },
                        )
                        .is_err()
                        {
                            return;
                        }
                        cursor = EventCursor {
                            xact: row.xact.clone(),
                            seq: row.seq,
                        };
                    }
                }
                Err(err) => {
                    // Fail closed: the credential could not be checked.
                    tracing::warn!("task stream poll failed: {}", err);
                    return;
                }
            }
        }
    });

    TaskAuthorizedSseStream {
        queue_rx,
        queue_body_bytes,
        auth,
        pending: None,
        authorize: None,
        _guard: guard,
    }
}

struct AccessSseStream {
    end_rx: tokio::sync::mpsc::Receiver<()>,
    _disconnect_rx: tokio::sync::mpsc::Receiver<()>,
    _guard: StreamGuard,
}

impl Stream for AccessSseStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut().end_rx.poll_recv(cx) {
            Poll::Ready(Some(())) | Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn access_sse_stream(
    hub: Arc<StreamHub>,
    pool: sqlx::PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    cursor: EventCursor,
    guard: StreamGuard,
) -> AccessSseStream {
    let (end_tx, end_rx) = tokio::sync::mpsc::channel(1);
    let (disconnect_tx, disconnect_rx) = tokio::sync::mpsc::channel::<()>(1);
    tokio::spawn(async move {
        let mut cursor = cursor;
        loop {
            if !hub.accepting() {
                break;
            }
            tokio::select! {
                _ = disconnect_tx.closed() => break,
                _ = tokio::time::sleep(STREAM_POLL_INTERVAL) => {}
            }
            // Credential, membership and access events share one transaction.
            match poll_access_events(&pool, workspace_id, user_id, session_id, &cursor).await {
                Ok(Some(next)) => cursor = next,
                Ok(None) => break,
                Err(err) => {
                    // Fail closed: the credential could not be checked.
                    tracing::warn!("access stream poll failed: {}", err);
                    break;
                }
            }
        }
        end_tx.try_send(()).ok();
    });
    AccessSseStream {
        end_rx,
        _disconnect_rx: disconnect_rx,
        _guard: guard,
    }
}

#[cfg(all(feature = "db-tests", test))]
mod queue_budget_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;

    use super::{release_queue_body_bytes, reserve_queue_body_bytes};

    #[test]
    fn reserve_rolls_back_when_channel_send_fails() {
        let budget = Arc::new(AtomicUsize::new(0));
        let bytes = 40;
        assert!(reserve_queue_body_bytes(&budget, bytes, 100));
        assert_eq!(budget.load(Ordering::Acquire), bytes);
        release_queue_body_bytes(&budget, bytes);
        assert_eq!(budget.load(Ordering::Acquire), 0);
    }

    #[test]
    fn concurrent_reserves_stay_within_limit() {
        let budget = Arc::new(AtomicUsize::new(0));
        let limit = 64;
        let chunk = 8;
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let budget = Arc::clone(&budget);
                thread::spawn(move || {
                    for _ in 0..8 {
                        reserve_queue_body_bytes(&budget, chunk, limit);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().expect("thread");
        }
        assert!(
            budget.load(Ordering::Acquire) <= limit,
            "budget must not exceed limit"
        );
    }
}
