use axum::http::HeaderMap;
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::auth::scopes::{grants_api_token_scope, ApiTokenScope};
use crate::auth::session::SessionUser;
use crate::auth::token::hash_token;
use crate::db::api_tokens::{resolve_api_token_session, ApiTokenSession};
use crate::error::{AppError, ProblemCode, SESSION_COOKIE};
use crate::http::state::AppState;

/// Which credentials a route admits. A session cookie passes every variant;
/// the variants differ only in the API tokens they admit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Session cookie only: every API token is refused.
    Session,
    /// A session, or an API token with at least one scope.
    Any,
    /// A session, or an API token that grants this scope.
    Scope(ApiTokenScope),
}

#[derive(Debug, Clone)]
pub struct RequestAuth {
    pub user: SessionUser,
    pub user_id: Uuid,
    /// The session id for a cookie, the token id for an API token: what a
    /// write re-checks under its fence (`db::context::recheck_session`).
    pub credential_id: Uuid,
    /// Present only for API tokens. Session auth keeps mixed content unfiltered.
    pub token_scopes: Option<Vec<ApiTokenScope>>,
    /// The workspace an API token is bound to; `None` for sessions.
    pub token_workspace_id: Option<Uuid>,
}

/// The Bearer path rule (source parity): 404 when the path, once
/// percent-decoded, has a `.` or `..` segment or does not decode (a bad
/// escape or invalid UTF-8). The query and fragment are not checked.
pub fn canonicalize_api_token_path(raw_path: &str) -> Result<(), AppError> {
    let cut = raw_path.split('#').next().unwrap_or(raw_path);
    let cut = cut.split('?').next().unwrap_or(cut);
    let decoded = percent_decode(cut).ok_or_else(|| AppError::from_code(ProblemCode::NotFound))?;
    if decoded.split('/').any(|part| part == "." || part == "..") {
        return Err(AppError::from_code(ProblemCode::NotFound));
    }
    Ok(())
}

fn percent_decode(value: &str) -> Option<String> {
    let mut out = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = from_hex(bytes[i + 1])?;
            let lo = from_hex(bytes[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get("authorization")?.to_str().ok()?.trim();
    value
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

fn parse_user_id(value: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(value).map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))
}

/// Entry authorization for a request: who is calling, and whether the
/// route's [`Access`] admits that credential.
///
/// A `fvoci_session` cookie, when present, decides alone: a stale cookie is
/// 401 even when a valid Bearer token is also sent. Without a cookie, a
/// `Bearer` API token is resolved. `workspace_id` binds a token to the
/// route's workspace; sessions get no workspace or membership check here,
/// the route or its DB call does that.
///
/// Side effects: a session's expiry may slide, and a token's `last_used_at`
/// is updated.
///
/// # Errors
///
/// - 401 `authentication_required`: no credential; a session or token that
///   is unknown, expired or revoked, or whose user is suspended or deleted;
///   a stale cookie even alongside a valid Bearer token.
/// - 404 `not_found`: a live token that `access` refuses (`Access::Session`,
///   a missing scope) or that is bound to another workspace, so a token
///   cannot tell a session-only route from a missing one.
/// - 500 on a database failure.
///
/// This is an entry check on the pool, not part of the caller's write
/// transaction, so it does not stop a write racing a logout, revocation or
/// suspension. A write passes [`RequestAuth::credential_id`] to its DB call,
/// which re-checks it after taking its fence (`db::context::recheck_session`).
pub async fn require_request_auth(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    access: Access,
    workspace_id: Option<Uuid>,
) -> Result<RequestAuth, AppError> {
    let cookie = jar.get(SESSION_COOKIE).map(|c| c.value().to_string());
    if let Some(token) = cookie {
        let user = state
            .auth
            .session_user(&token)
            .await
            .map_err(internal)?
            .ok_or_else(|| AppError::from_code(ProblemCode::AuthenticationRequired))?;
        let session_id = Uuid::parse_str(&user.session_id)
            .map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))?;
        let user_id = parse_user_id(&user.user_id)?;
        return Ok(RequestAuth {
            user,
            user_id,
            credential_id: session_id,
            token_scopes: None,
            token_workspace_id: None,
        });
    }

    let Some(raw) = bearer_token(headers) else {
        return Err(AppError::from_code(ProblemCode::AuthenticationRequired));
    };
    let resolved = resolve_api_token_session(&state.auth.db.pool, &hash_token(raw))
        .await
        .map_err(internal)?;
    apply_token_access(resolved, access, workspace_id)
}

fn apply_token_access(
    resolved: Option<ApiTokenSession>,
    access: Access,
    workspace_id: Option<Uuid>,
) -> Result<RequestAuth, AppError> {
    let Some(token) = resolved else {
        return Err(AppError::from_code(ProblemCode::AuthenticationRequired));
    };
    if matches!(access, Access::Session) || token.scopes.is_empty() {
        return Err(AppError::from_code(ProblemCode::NotFound));
    }
    if let Access::Scope(required) = access {
        if !grants_api_token_scope(&token.scopes, required) {
            return Err(AppError::from_code(ProblemCode::NotFound));
        }
    }
    if let Some(workspace_id) = workspace_id {
        if workspace_id != token.workspace_id {
            return Err(AppError::from_code(ProblemCode::NotFound));
        }
    }
    let user_id = parse_user_id(&token.user.user_id)?;
    Ok(RequestAuth {
        user: token.user,
        user_id,
        credential_id: token.token_id,
        token_scopes: Some(token.scopes),
        token_workspace_id: Some(token.workspace_id),
    })
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_rejects_dot_segments() {
        assert!(canonicalize_api_token_path("/api/v1/workspaces/x/documents").is_ok());
        assert!(canonicalize_api_token_path("/api/v1/../secret").is_err());
        assert!(canonicalize_api_token_path("/api/v1/%2e%2e/secret").is_err());
    }
}
