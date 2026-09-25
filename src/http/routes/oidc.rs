//! OIDC sign-in, identity links and workspace SSO configuration (source
//! `apps/server/src/domains/identity/oidc.ts`, `domains/workspaces/oidc.ts`).
//!
//! Workspace SSO is an EE feature in the source (`workspaceSso`); license
//! verification is not ported, so, like branding and audit, it is enabled.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::dto::{
    IdentitiesOutput, IdentityOutput, OkResponse, ProviderOutput, ProvidersOutput,
    WorkspaceOidcBody, WorkspaceOidcGetOutput, WorkspaceOidcOutput,
};
use crate::auth::scopes::ApiTokenScope;
use crate::db::oidc::{self as db, ManageError, UnlinkOutcome, WorkspaceOidcInput};
use crate::error::{AppError, ProblemCode, SESSION_COOKIE};
use crate::http::authz::{require_request_auth, Access, RequestAuth};
use crate::http::cookie::set_session_cookie;
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::identity::{workspace_oidc_context, Identity};
use crate::oidc::flow::{
    self, BeginError, BeginParams, CompleteParams, Mode, OidcResult, Started, STATE_COOKIE,
    STATE_TTL_SECS,
};
use crate::oidc::providers::{normalize_issuer, ProviderKey};
use crate::secret_box;

/// Source `AUTH_RATE_LIMIT.oidcPerIp` (5-minute window).
const OIDC_PER_IP: u32 = 30;
const ISSUER_MAX: usize = 2048;
const CLIENT_ID_MAX: usize = 256;
const CLIENT_SECRET_MAX: usize = 4096;
const LABEL_MAX: usize = 100;
const DEFAULT_LABEL: &str = "SSO";

pub fn router(identity: Arc<Identity>) -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/providers", get(providers))
        .route("/api/v1/auth/identities", get(identities))
        .route("/api/v1/auth/sso", get(sso))
        .route("/api/v1/auth/oidc/{provider}/start", get(start))
        .route("/api/v1/auth/oidc/{provider}/callback", get(callback))
        .route("/api/v1/auth/oidc/{provider}/link", post(link))
        .route("/api/v1/auth/oidc/{provider}/unlink", post(unlink))
        .route(
            "/api/v1/workspaces/{workspace_id}/oidc",
            get(get_workspace_oidc)
                .put(put_workspace_oidc)
                .delete(delete_workspace_oidc),
        )
        .layer(Extension(identity))
}

fn internal(err: sqlx::Error) -> AppError {
    let message = match &err {
        sqlx::Error::Database(db) => db.message().to_string(),
        _ => "database operation failed".to_string(),
    };
    tracing::error!("database error: {message}");
    AppError::internal()
}

async fn limit_ip(state: &AppState, peer: SocketAddr) -> Result<(), AppError> {
    let ip = peer_ip(peer.ip());
    state
        .rate_limiter
        .allow(&format!("oidc-ip:{ip}"), OIDC_PER_IP)
        .await
        .map_err(AppError::rate_limited)
}

fn provider_param(raw: &str) -> Result<ProviderKey, AppError> {
    ProviderKey::parse(raw)
        .ok_or_else(|| AppError::with_source(ProblemCode::InvalidInput, "/provider"))
}

/// Strict query (source `z.strictObject`): known keys once each.
fn strict_query(
    raw: Option<String>,
    allowed: &[&str],
) -> Result<HashMap<String, String>, AppError> {
    let mut out = HashMap::new();
    for (key, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        if !allowed.contains(&key.as_ref()) || out.contains_key(key.as_ref()) {
            return Err(AppError::with_source(ProblemCode::InvalidInput, "/"));
        }
        out.insert(key.into_owned(), value.into_owned());
    }
    Ok(out)
}

fn uuid_query(query: &HashMap<String, String>, key: &str) -> Result<Option<Uuid>, AppError> {
    query
        .get(key)
        .map(|v| {
            Uuid::parse_str(v).map_err(|_| AppError::with_source(ProblemCode::InvalidInput, "/"))
        })
        .transpose()
}

#[derive(Deserialize)]
struct ConsentQueryItem {
    kind: String,
    version: i32,
}

/// Source `parseConsentsQuery`: JSON array of `{kind, version}`.
fn parse_consents(raw: Option<&String>) -> Result<Vec<(String, i32)>, AppError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let items: Vec<ConsentQueryItem> = serde_json::from_str(raw)
        .map_err(|_| AppError::from_code(ProblemCode::InvalidConsentsQuery))?;
    Ok(items.into_iter().map(|i| (i.kind, i.version)).collect())
}

fn state_cookie(secure: bool, value: &str, max_age: i64) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{STATE_COOKIE}={value}; HttpOnly; Path=/; SameSite=Lax; Max-Age={max_age}{secure}")
}

fn append_cookie(response: &mut Response, cookie: &str) {
    if let Ok(value) = HeaderValue::from_str(cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

fn redirect(status: StatusCode, location: &str) -> Response {
    let mut response = status.into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

fn state_redirect(state: &AppState, started: Started, status: StatusCode) -> Response {
    let mut response = redirect(status, &started.authorization_url);
    append_cookie(
        &mut response,
        &state_cookie(state.cookie_secure, &started.signed_state, STATE_TTL_SECS),
    );
    response
}

fn begin_error(err: BeginError) -> AppError {
    match err {
        BeginError::NotConfigured => AppError::from_code(ProblemCode::ProviderNotConfigured),
        BeginError::Unavailable => AppError::from_code(ProblemCode::EncryptionUnavailable),
        BeginError::Provider(reason) => {
            tracing::warn!(%reason, "oidc.begin_failed");
            AppError::internal()
        }
        BeginError::Db(err) => internal(err),
    }
}

async fn providers(
    State(state): State<AppState>,
    Extension(identity): Extension<Arc<Identity>>,
) -> Result<Json<ProvidersOutput>, AppError> {
    let workspace_sso = identity.encryption_keys.is_some()
        && db::any_workspace_oidc(&state.auth.db.pool)
            .await
            .map_err(internal)?;
    Ok(Json(ProvidersOutput {
        providers: identity
            .oidc
            .providers
            .iter()
            .map(|p| ProviderOutput {
                provider: p.key.as_str().to_string(),
                label: p.label.clone(),
            })
            .collect(),
        magic_link: state.mailer.enabled(),
        workspace_sso,
    }))
}

async fn session(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
) -> Result<RequestAuth, AppError> {
    require_request_auth(state, headers, jar, Access::Session, None).await
}

async fn identities(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<IdentitiesOutput>, AppError> {
    let auth = session(&state, &headers, &jar).await?;
    let links = db::list_links(&state.auth.db.pool, auth.user_id)
        .await
        .map_err(internal)?;
    Ok(Json(IdentitiesOutput {
        items: links
            .into_iter()
            .map(|l| IdentityOutput {
                provider: l.provider,
                email: l.email,
                created_at: l.created_at,
            })
            .collect(),
    }))
}

async fn sso(
    State(state): State<AppState>,
    Extension(identity): Extension<Arc<Identity>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RawQuery(raw): RawQuery,
) -> Result<Response, AppError> {
    limit_ip(&state, peer).await?;
    let query = strict_query(raw, &["slug"])?;
    let slug = query
        .get("slug")
        .ok_or_else(|| AppError::with_source(ProblemCode::InvalidInput, "/slug"))?;
    let slug = crate::validate::normalize_slug(slug.trim())?;
    let Some(workspace_id) = db::sso_workspace_by_slug(&state.auth.db.pool, &slug)
        .await
        .map_err(internal)?
    else {
        return Err(AppError::from_code(ProblemCode::ProviderNotConfigured));
    };
    let started = flow::begin(
        &state.auth.db.pool,
        &identity,
        BeginParams {
            provider: ProviderKey::Generic,
            mode: Mode::Login,
            invitation_token: None,
            user_id: None,
            consents: Vec::new(),
            workspace_id: Some(workspace_id),
        },
    )
    .await
    .map_err(begin_error)?;
    Ok(state_redirect(&state, started, StatusCode::FOUND))
}

async fn start(
    State(state): State<AppState>,
    Extension(identity): Extension<Arc<Identity>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(provider): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, AppError> {
    let provider = provider_param(&provider)?;
    limit_ip(&state, peer).await?;
    let query = strict_query(raw, &["invitation", "consents", "workspaceId"])?;
    let invitation = query.get("invitation").cloned();
    if invitation.as_deref() == Some("") {
        return Err(AppError::with_source(
            ProblemCode::InvalidInput,
            "/invitation",
        ));
    }
    let consents = parse_consents(query.get("consents"))?;
    let workspace_id = uuid_query(&query, "workspaceId")?;
    let started = flow::begin(
        &state.auth.db.pool,
        &identity,
        BeginParams {
            provider,
            mode: if invitation.is_some() {
                Mode::Invite
            } else {
                Mode::Login
            },
            invitation_token: invitation,
            user_id: None,
            consents,
            workspace_id,
        },
    )
    .await
    .map_err(begin_error)?;
    Ok(state_redirect(&state, started, StatusCode::FOUND))
}

async fn callback(
    State(state): State<AppState>,
    Extension(identity): Extension<Arc<Identity>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(provider): Path<String>,
    jar: CookieJar,
    RawQuery(raw): RawQuery,
) -> Result<Response, AppError> {
    let provider = provider_param(&provider)?;
    limit_ip(&state, peer).await?;
    let mut query = HashMap::new();
    for (key, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        query
            .entry(key.into_owned())
            .or_insert_with(|| value.into_owned());
    }
    let signed_state = jar.get(STATE_COOKIE).map(|c| c.value().to_string());
    let session_user_id = match jar.get(SESSION_COOKIE) {
        Some(cookie) => state
            .auth
            .session_user(cookie.value())
            .await
            .map_err(internal)?
            .and_then(|u| Uuid::parse_str(&u.user_id).ok()),
        None => None,
    };
    let settings = crate::settings::current_values(&state.auth.db.pool, &state.branding_name)
        .await
        .map_err(internal)?;
    let ip = peer_ip(peer.ip());
    let result = flow::complete(
        &state.auth.db.pool,
        &identity,
        CompleteParams {
            provider,
            query: &query,
            signed_state: signed_state.as_deref(),
            session_user_id,
            ip: Some(&ip),
            defaults: &settings.defaults_user,
        },
    )
    .await;
    let origin = state.public_origin.trim_end_matches('/');
    let clear = state_cookie(state.cookie_secure, "", 0);
    let (location, session_token) = match result {
        Ok(OidcResult::Session { token, .. }) => (format!("{origin}/"), Some(token)),
        // The pending token rides in the fragment: it never reaches server
        // logs or a Referer, and the login screen clears it at once.
        Ok(OidcResult::Mfa { mfa_token }) => (format!("{origin}/login#mfa={mfa_token}"), None),
        Ok(OidcResult::Linked) => (format!("{origin}/settings/account?linked=1"), None),
        Ok(OidcResult::Error { code, mode }) => {
            tracing::warn!(
                provider = provider.as_str(),
                reason = code.as_str(),
                mode = ?mode,
                "oidc.callback_failed"
            );
            let path = if mode == Some(Mode::Link) {
                "/settings/account"
            } else {
                "/login"
            };
            (format!("{origin}{path}?error={}", code.as_str()), None)
        }
        Ok(OidcResult::SeatLimit) => {
            let mut response = AppError::from_code(ProblemCode::LimitSeats).into_response();
            append_cookie(&mut response, &clear);
            return Ok(response);
        }
        Err(err) => {
            let message = match &err {
                sqlx::Error::Database(db) => db.message().to_string(),
                _ => "database operation failed".to_string(),
            };
            tracing::error!(provider = provider.as_str(), reason = %message, "oidc.callback_failed");
            (format!("{origin}/login?error=oidc_provider_error"), None)
        }
    };
    let mut response = redirect(StatusCode::FOUND, &location);
    append_cookie(&mut response, &clear);
    if let Some(token) = session_token {
        append_cookie(
            &mut response,
            &set_session_cookie(state.cookie_secure, &token),
        );
    }
    Ok(response)
}

async fn link(
    State(state): State<AppState>,
    Extension(identity): Extension<Arc<Identity>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = session(&state, &headers, &jar).await?;
    let provider = provider_param(&provider)?;
    limit_ip(&state, peer).await?;
    let query = strict_query(raw, &["workspaceId"])?;
    let workspace_id = uuid_query(&query, "workspaceId")?;
    let started = flow::begin(
        &state.auth.db.pool,
        &identity,
        BeginParams {
            provider,
            mode: Mode::Link,
            invitation_token: None,
            user_id: Some(auth.user_id),
            consents: Vec::new(),
            workspace_id,
        },
    )
    .await
    .map_err(begin_error)?;
    Ok(state_redirect(&state, started, StatusCode::SEE_OTHER))
}

async fn unlink(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider): Path<String>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = session(&state, &headers, &jar).await?;
    let provider = provider_param(&provider)?;
    match db::unlink(
        &state.auth.db.pool,
        auth.user_id,
        auth.credential_id,
        provider.as_str(),
        state.mailer.enabled(),
    )
    .await
    .map_err(internal)?
    {
        UnlinkOutcome::Ok => Ok(Json(OkResponse { ok: true })),
        UnlinkOutcome::NotFound => Err(AppError::from_code(ProblemCode::IdentityLinkNotFound)),
        UnlinkOutcome::LastMethod => Err(AppError::from_code(ProblemCode::OidcLastMethod)),
        UnlinkOutcome::SessionGone => Err(AppError::from_code(ProblemCode::AuthenticationRequired)),
    }
}

// ---------------------------------------------------------------------------
// Workspace SSO configuration (workspace.manage)

async fn manage_auth(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
) -> Result<RequestAuth, AppError> {
    require_request_auth(
        state,
        headers,
        jar,
        Access::Scope(ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await
}

fn manage_error(err: ManageError) -> AppError {
    match err {
        ManageError::NotFound => AppError::from_code(ProblemCode::NotFound),
        ManageError::Forbidden => AppError::from_code(ProblemCode::InsufficientPermissions),
        ManageError::SessionGone => AppError::from_code(ProblemCode::AuthenticationRequired),
    }
}

fn workspace_path(raw: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(raw)
        .map_err(|_| AppError::with_source(ProblemCode::InvalidInput, "/workspaceId"))
}

async fn get_workspace_oidc(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<String>,
) -> Result<Json<WorkspaceOidcGetOutput>, AppError> {
    let workspace_id = workspace_path(&workspace_id)?;
    let auth = manage_auth(&state, &headers, &jar, workspace_id).await?;
    let row = db::get_workspace_oidc(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
    )
    .await
    .map_err(internal)?
    .map_err(manage_error)?;
    Ok(Json(match row {
        Some(row) => WorkspaceOidcGetOutput {
            issuer: Some(row.issuer),
            client_id: Some(row.client_id),
            label: Some(row.label),
        },
        None => WorkspaceOidcGetOutput {
            issuer: None,
            client_id: None,
            label: None,
        },
    }))
}

/// Source `workspaceOidcInput` + `normalizeIssuer` / `normalizeLabel`.
fn validate_input(body: &WorkspaceOidcBody) -> Result<(String, String, String), AppError> {
    let invalid =
        |field: &str| AppError::with_source(ProblemCode::InvalidInput, format!("/{field}"));
    let issuer = normalize_issuer(&body.issuer);
    if issuer.is_empty() || issuer.chars().count() > ISSUER_MAX {
        return Err(invalid("issuer"));
    }
    let parsed = url::Url::parse(&issuer).map_err(|_| invalid("issuer"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
        return Err(invalid("issuer"));
    }
    let client_id = body.client_id.trim().to_string();
    if client_id.is_empty() || client_id.chars().count() > CLIENT_ID_MAX {
        return Err(invalid("clientId"));
    }
    if body.client_secret.is_empty() || body.client_secret.chars().count() > CLIENT_SECRET_MAX {
        return Err(invalid("clientSecret"));
    }
    let label = body.label.as_deref().map(str::trim).unwrap_or_default();
    if label.chars().count() > LABEL_MAX {
        return Err(invalid("label"));
    }
    let label = if label.is_empty() {
        DEFAULT_LABEL
    } else {
        label
    };
    Ok((issuer, client_id, label.to_string()))
}

async fn put_workspace_oidc(
    State(state): State<AppState>,
    Extension(identity): Extension<Arc<Identity>>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<String>,
    body: Result<Json<WorkspaceOidcBody>, JsonRejection>,
) -> Result<Json<WorkspaceOidcOutput>, AppError> {
    let workspace_id = workspace_path(&workspace_id)?;
    check_origin(&headers, &state.public_origin)?;
    let auth = manage_auth(&state, &headers, &jar, workspace_id).await?;
    let Json(body) = body.map_err(AppError::from)?;
    let (issuer, client_id, label) = validate_input(&body)?;
    let Some(keys) = identity.encryption_keys.as_deref() else {
        tracing::warn!("workspace oidc refused: ENCRYPTION_KEYS is not configured");
        return Err(AppError::from_code(ProblemCode::EncryptionUnavailable));
    };
    let sealed = secret_box::seal(
        keys,
        &body.client_secret,
        &workspace_oidc_context(workspace_id),
    )
    .map_err(|_| AppError::internal())?;
    let row = db::upsert_workspace_oidc(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        WorkspaceOidcInput {
            issuer: &issuer,
            client_id: &client_id,
            sealed_secret: &sealed,
            label: &label,
        },
    )
    .await
    .map_err(internal)?
    .map_err(manage_error)?;
    Ok(Json(WorkspaceOidcOutput {
        issuer: row.issuer,
        client_id: row.client_id,
        label: row.label,
    }))
}

async fn delete_workspace_oidc(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<String>,
) -> Result<Json<OkResponse>, AppError> {
    let workspace_id = workspace_path(&workspace_id)?;
    check_origin(&headers, &state.public_origin)?;
    let auth = manage_auth(&state, &headers, &jar, workspace_id).await?;
    db::remove_workspace_oidc(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
    )
    .await
    .map_err(internal)?
    .map_err(manage_error)?;
    Ok(Json(OkResponse { ok: true }))
}
