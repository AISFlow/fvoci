//! Intent capture is session-owned, private by construction and atomic in Rust.
use crate::api::personal_input_dto::{PersonalInputBody, PersonalInputIntent, PersonalInputOutput};
use crate::db::personal_input::{create_personal_input, PersonalInputDbError};
use crate::error::{AppError, ProblemCode};
use crate::http::routes::{
    task_body::map_origin_error,
    tasks::{activity_channel, internal, map_task_db_error, TaskApiError},
};
use crate::http::{
    authz::{require_request_auth, Access},
    guard::check_origin,
    rate_limit::peer_ip,
    state::AppState,
};
use axum::{
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use axum_extra::extract::CookieJar;
use std::net::SocketAddr;
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/workspaces/{workspace_id}/personal-input",
        post(create),
    )
}
fn valid(input: &PersonalInputBody) -> bool {
    crate::db::documents::title_is_valid(input.title.trim())
        && (input.intent == PersonalInputIntent::Task
            || (input.source.is_none() && input.project_id.is_none()))
        && input.source.as_ref().is_none_or(|source| {
            source.anchor.as_ref().is_none_or(|anchor| {
                !anchor.is_empty()
                    && anchor.chars().count()
                        <= crate::db::task_origins::TASK_ORIGIN_ANCHOR_MAX_CHARS
            })
        })
}
async fn create(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Bytes,
) -> Result<(StatusCode, Json<PersonalInputOutput>), TaskApiError> {
    check_origin(&headers, &state.public_origin)?;
    let input: PersonalInputBody = serde_json::from_slice(&body)
        .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    if !valid(&input) {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Session, Some(workspace_id)).await?;
    let ip = peer_ip(peer.ip());
    let output = create_personal_input(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/personal_input.rs")
            .map_err(internal)?,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        &input,
        Some(&ip),
        activity_channel(&headers),
    )
    .await
    .map_err(internal)?
    .map_err(|err| match err {
        PersonalInputDbError::NotFound => AppError::from_code(ProblemCode::NotFound).into(),
        PersonalInputDbError::Forbidden => {
            AppError::from_code(ProblemCode::InsufficientPermissions).into()
        }
        PersonalInputDbError::RequestMismatch => {
            map_origin_error(crate::db::task_origins::TaskOriginDbError::RequestMismatch)
        }
        PersonalInputDbError::Project(err) => map_task_db_error(err),
        PersonalInputDbError::Origin(err) => map_origin_error(err),
    })?;
    Ok((StatusCode::CREATED, Json(output)))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_note_task_options_and_blank_input() {
        let mut input: PersonalInputBody = serde_json::from_value(
            serde_json::json!({"requestId":Uuid::nil(),"intent":"note","title":"한글 🙂"}),
        )
        .unwrap();
        assert!(valid(&input));
        input.project_id = Some(Uuid::nil());
        assert!(!valid(&input));
        input.intent = PersonalInputIntent::Task;
        assert!(valid(&input));
        input.title = " ".into();
        assert!(!valid(&input));
    }
}
