//! GET /api/v1/workspaces/{workspace_id}/events — workspace event log
//! (source apps/server/src/domains/events/routes.ts).

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use axum_extra::extract::CookieJar;
use serde_json::Value;
use uuid::Uuid;

use crate::api::dto::{WorkspaceEventListQuery, WorkspaceEventListResponse, WorkspaceEventOutput};
use crate::db::workspace_events::{
    decode_event_cursor, encode_event_cursor, list_workspace_events,
};
use crate::error::{AppError, ProblemCode};
use crate::http::state::AppState;

use super::{internal, map_workspace_error, require_session};

/// Source `z.coerce.number().int().min(1).max(100).default(50)`.
fn parse_limit(raw: Option<&str>) -> Result<i64, AppError> {
    let Some(raw) = raw else { return Ok(50) };
    // z.coerce.number(): Number("") is 0 and whitespace trims.
    let trimmed = raw.trim();
    let value: f64 = if trimmed.is_empty() {
        0.0
    } else {
        trimmed
            .parse()
            .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?
    };
    if value.fract() != 0.0 || !(1.0..=100.0).contains(&value) {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    Ok(value as i64)
}

fn invalid_cursor() -> AppError {
    AppError {
        status: StatusCode::BAD_REQUEST,
        code: ProblemCode::InvalidInput,
        source: None,
        params: Some(serde_json::json!({ "code": "invalid_cursor" })),
        retry_after: None,
    }
}

pub(super) async fn list_workspace_events_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    query: Result<Query<WorkspaceEventListQuery>, QueryRejection>,
) -> Result<Json<WorkspaceEventListResponse>, AppError> {
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let Query(query) = query.map_err(AppError::from)?;
    let limit = parse_limit(query.limit.as_deref())?;
    let cursor = match query.cursor.as_deref() {
        None => None,
        Some(raw) if raw.len() > 1024 => {
            return Err(AppError::from_code(ProblemCode::InvalidInput))
        }
        Some(raw) => Some(decode_event_cursor(raw).ok_or_else(invalid_cursor)?),
    };
    let page = list_workspace_events(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        cursor,
        limit,
    )
    .await
    .map_err(internal)?
    .map_err(|err| map_workspace_error(err, false))?;
    Ok(Json(WorkspaceEventListResponse {
        items: page
            .items
            .into_iter()
            .map(|row| WorkspaceEventOutput {
                id: row.id.to_string(),
                verb: row.verb,
                workspace_id: row.workspace_id.map(|id| id.to_string()),
                actor_user_id: row.actor_user_id.map(|id| id.to_string()),
                target_type: row.target_type,
                target_id: row.target_id.map(|id| id.to_string()),
                payload: match row.payload {
                    Value::Object(map) => Value::Object(map),
                    _ => Value::Object(Default::default()),
                },
                channel: row.channel,
                created_at: row.created_at,
            })
            .collect(),
        next_cursor: page.next_cursor.map(encode_event_cursor),
    }))
}

#[cfg(test)]
mod tests {
    use super::parse_limit;

    #[test]
    fn limit_coerces_like_zod() {
        assert_eq!(parse_limit(None).unwrap(), 50);
        assert_eq!(parse_limit(Some(" 7 ")).unwrap(), 7);
        assert_eq!(parse_limit(Some("100")).unwrap(), 100);
        for bad in ["", "0", "101", "1.5", "abc", "-1"] {
            assert!(parse_limit(Some(bad)).is_err(), "{bad}");
        }
    }
}
