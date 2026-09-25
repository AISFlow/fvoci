use axum::http::HeaderMap;
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::auth::scopes::{grants_api_token_scope, ApiTokenScope};
use crate::auth::session::SessionUser;
use crate::auth::token::hash_token;
use crate::db::api_tokens::{resolve_api_token_session, ApiTokenSession};
use crate::error::{AppError, ProblemCode, SESSION_COOKIE};
use crate::http::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Session,
    Any,
    Scope(ApiTokenScope),
}

#[derive(Debug, Clone)]
pub struct RequestAuth {
    pub user: SessionUser,
    pub user_id: Uuid,
    pub credential_id: Uuid,
}

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
