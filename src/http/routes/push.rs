//! `PUT /api/v1/workspaces/{id}/push-subscriptions` (source
//! `routes.workspaces.pushSubscriptions`): session only (no API tokens),
//! guest+ membership, user-global row. There is no DELETE route: the browser
//! unsubscribes and the sender removes the endpoint on 404/410.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::put;
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::api::dto::{OkResponse, PushSubscriptionBody};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access};
use crate::http::guard::check_origin;
use crate::http::state::AppState;
use crate::integrations::outbound::{parse_target_url, OutboundPolicy};
use crate::push::db::{normalize_subscription_key, register_subscription, PushSubscriptionRow};

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/workspaces/{workspace_id}/push-subscriptions",
        put(put_push_subscription),
    )
}

/// https only (source contract), then the outbound URL rules used at send
/// time: no userinfo, port absent/80/443, no blocked names or private
/// literals. DNS is checked again when sending.
fn validate_endpoint(raw: &str) -> Option<String> {
    let url = parse_target_url(raw, &OutboundPolicy::default()).ok()?;
    (url.scheme() == "https" && raw.trim() == raw).then(|| raw.to_string())
}

fn validate_body(body: PushSubscriptionBody) -> Result<PushSubscriptionRow, AppError> {
    let invalid = || AppError::from_code(ProblemCode::InvalidInput);
    Ok(PushSubscriptionRow {
        endpoint: validate_endpoint(&body.endpoint).ok_or_else(invalid)?,
        p256dh: normalize_subscription_key(&body.keys.p256dh, 65).ok_or_else(invalid)?,
        auth: normalize_subscription_key(&body.keys.auth, 16).ok_or_else(invalid)?,
    })
}

async fn put_push_subscription(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<PushSubscriptionBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Session, Some(workspace_id)).await?;
    let Json(body) = body.map_err(AppError::from)?;
    let subscription = validate_body(body)?;
    let stored = register_subscription(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/push.rs")
            .map_err(crate::http::routes::tasks::internal)?,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        &subscription,
    )
    .await
    .map_err(|err| {
        tracing::error!("database error: {err}");
        AppError::internal()
    })?;
    if !stored {
        return Err(AppError::from_code(ProblemCode::NotFound));
    }
    Ok(Json(OkResponse { ok: true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_must_be_public_https() {
        assert!(validate_endpoint("https://fcm.googleapis.com/fcm/send/abc").is_some());
        assert!(
            validate_endpoint("https://updates.push.services.mozilla.com:443/wpush/v2/x").is_some()
        );
        for bad in [
            "http://push.example.com/x",
            "https://user:pw@push.example.com/x",
            "https://push.example.com:8443/x",
            "https://localhost/x",
            "https://127.0.0.1/x",
            "https://[::1]/x",
            "https://10.0.0.1/x",
            "https://metadata.google.internal/x",
            " https://push.example.com/x",
            "ftp://push.example.com/x",
            "not a url",
        ] {
            assert!(validate_endpoint(bad).is_none(), "{bad}");
        }
    }
}
