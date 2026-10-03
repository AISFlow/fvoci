//! Same-origin confirmed transfers; previews are authorized and effect-free.
use crate::api::personal_transfer::{
    PersonalTransferAction, PersonalTransferBody, PersonalTransferOutput, PersonalTransferPreview,
    PersonalTransferSelection,
};
use crate::collab::room::RoomKey;
use crate::db::personal_transfer::{
    preview_personal_transfer, transfer_personal_item, PersonalTransferDbError, TransferBodyEngine,
    TransferFiles,
};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::routes::tasks::{activity_channel, internal, TaskApiError};
use crate::http::state::AppState;
use axum::{
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use axum_extra::extract::CookieJar;
use serde::Serialize;
use serde_json::json;
use std::net::SocketAddr;
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/personal-transfers/preview",
            post(preview),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/personal-transfers",
            post(transfer),
        )
}
/// The shared coded problem plus a typed `params.code`, so clients branch on
/// the stable enum and never on the diagnostic title.
pub(crate) enum TransferApiError {
    Task(TaskApiError),
    Typed {
        code: &'static str,
        title: String,
        reason: serde_json::Value,
    },
}
impl From<TaskApiError> for TransferApiError {
    fn from(value: TaskApiError) -> Self {
        Self::Task(value)
    }
}
impl From<AppError> for TransferApiError {
    fn from(value: AppError) -> Self {
        Self::Task(value.into())
    }
}
impl IntoResponse for TransferApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Task(error) => error.into_response(),
            Self::Typed {
                code,
                title,
                reason,
            } => {
                let status = StatusCode::CONFLICT;
                let body = json!({
                    "type": "about:blank",
                    "title": title,
                    "status": status.as_u16(),
                    "code": code,
                    "params": { "code": reason },
                });
                let mut headers = HeaderMap::new();
                headers.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/problem+json"),
                );
                (status, headers, Json(body)).into_response()
            }
        }
    }
}
fn wire<T: Serialize>(value: T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}
fn map_error(error: PersonalTransferDbError) -> TransferApiError {
    match error {
        PersonalTransferDbError::NotFound => AppError::from_code(ProblemCode::NotFound).into(),
        PersonalTransferDbError::Forbidden => {
            AppError::from_code(ProblemCode::InsufficientPermissions).into()
        }
        PersonalTransferDbError::InvalidInput => {
            AppError::from_code(ProblemCode::InvalidInput).into()
        }
        PersonalTransferDbError::Conflict(reason) => TransferApiError::Typed {
            code: "personal_transfer_conflict",
            title: "transfer command or preview changed".into(),
            reason: wire(reason),
        },
        PersonalTransferDbError::Incomplete(blocker, diagnostic) => TransferApiError::Typed {
            code: "personal_transfer_incomplete",
            title: diagnostic.into(),
            reason: wire(blocker),
        },
    }
}
fn body_engine(state: &AppState) -> Option<TransferBodyEngine> {
    state
        .collab
        .as_ref()
        .map(|hub| TransferBodyEngine {
            engine_bin: hub.engine_bin(),
            limits: hub.limits(),
        })
        .or_else(|| {
            crate::collab::CollabConfig::from_env().map(|config| TransferBodyEngine {
                engine_bin: config.engine_bin,
                limits: config.limits,
            })
        })
}
async fn preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(source): Path<Uuid>,
    body: Bytes,
) -> Result<Json<PersonalTransferPreview>, TransferApiError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = require_request_auth(&state, &headers, &jar, Access::Session, Some(source)).await?;
    let selection: PersonalTransferSelection = serde_json::from_slice(&body)
        .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    let output = preview_personal_transfer(
        &state.auth.db.pool,
        source,
        auth.user_id,
        auth.credential_id,
        &selection,
        body_engine(&state).as_ref(),
    )
    .await
    .map_err(|error| TransferApiError::from(internal(error)))?
    .map_err(map_error)?;
    Ok(Json(output))
}
async fn transfer(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(source): Path<Uuid>,
    body: Bytes,
) -> Result<Json<PersonalTransferOutput>, TransferApiError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = require_request_auth(&state, &headers, &jar, Access::Session, Some(source)).await?;
    let command: PersonalTransferBody = serde_json::from_slice(&body)
        .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    if !command.confirmed {
        return Err(AppError::from_code(ProblemCode::ConfirmInvalid).into());
    }
    let ip = peer_ip(peer.ip());
    let output = transfer_personal_item(
        &state.auth.db.pool,
        source,
        auth.user_id,
        auth.credential_id,
        &command,
        Some(&ip),
        activity_channel(&headers),
        body_engine(&state).as_ref(),
        Some(TransferFiles {
            storage: &state.storage,
            quota: &state.quota,
        }),
    )
    .await
    .map_err(|error| TransferApiError::from(internal(error)))?
    .map_err(map_error)?;
    // A committed same-ID MOVE retires the moved resources' source rooms
    // before success, so the destination room can start at once. COPY and
    // every refusal returned above never retire anything.
    if command.selection.action == PersonalTransferAction::Move {
        if let Some(hub) = &state.collab {
            let mut keys = vec![RoomKey::document(source, output.document_id)];
            keys.extend(output.task_id.map(|task| RoomKey::task(source, task)));
            hub.retire_moved_resource_rooms(keys).await;
        }
    }
    Ok(Json(output))
}
