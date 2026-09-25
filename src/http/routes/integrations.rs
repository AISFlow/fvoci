//! Integration routes (source `domains/integrations/{webhooks,github,ai}.ts`).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, JsonRejection};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::dto::{
    AiDocumentBody, AiGenerateTasksOutput, AiSuggestLinksOutput, AiSummarizeOutput,
    GithubInstallOutput, GithubInstallUrlOutput, GithubIssueLinkBody, GithubIssueLinkOutput,
    OkResponse, WebhookCreateBody, WebhookCreatedOutput, WebhookListResponse, WebhookOutput,
};
use crate::auth::scopes::ApiTokenScope;
use crate::db::integrations::{
    create_webhook, list_webhooks, remove_webhook, IntegrationDbError, NewWebhook, WebhookRow,
};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::integrations::github::{self, fetch_app_slug, InboundError, GITHUB_WEBHOOK_BODY_MAX};
use crate::integrations::outbound::parse_target_url;
use crate::integrations::webhooks::{normalize_events, webhook_secret_context};
use crate::integrations::{ai, Integrations};
use crate::secret_box;

pub fn router(integrations: Arc<Integrations>) -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/webhooks",
            get(list_webhooks_route).post(create_webhook_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}",
            axum::routing::delete(remove_webhook_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/github",
            get(get_github_route).delete(remove_github_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/github/install",
            post(install_github_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/github/issue-links",
            post(link_issue_route),
        )
        .route("/api/v1/github/callback", get(github_callback_route))
        .route(
            "/api/v1/github/webhook",
            post(github_webhook_route).layer(DefaultBodyLimit::max(GITHUB_WEBHOOK_BODY_MAX)),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/ai/summarize",
            post(ai_summarize_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/ai/generate-tasks",
            post(ai_generate_tasks_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/ai/suggest-links",
            post(ai_suggest_links_route),
        )
        .layer(Extension(integrations))
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}

fn map_db_error(err: IntegrationDbError) -> AppError {
    match err {
        IntegrationDbError::NotFound => AppError::from_code(ProblemCode::NotFound),
        IntegrationDbError::Conflict | IntegrationDbError::InvalidInput => {
            AppError::from_code(ProblemCode::InvalidInput)
        }
    }
}

async fn auth(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    scope: ApiTokenScope,
    workspace_id: Uuid,
) -> Result<(Uuid, Uuid), AppError> {
    let auth = require_request_auth(
        state,
        headers,
        jar,
        Access::Scope(scope),
        Some(workspace_id),
    )
    .await?;
    Ok((auth.user_id, auth.credential_id))
}

fn webhook_output(row: WebhookRow) -> WebhookOutput {
    WebhookOutput {
        id: row.id.to_string(),
        workspace_id: row.workspace_id.to_string(),
        url: row.url,
        events: row.events,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

async fn list_webhooks_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<WebhookListResponse>, AppError> {
    let (user_id, session_id) = auth(
        &state,
        &headers,
        &jar,
        ApiTokenScope::WorkspaceManage,
        workspace_id,
    )
    .await?;
    let rows = list_webhooks(&state.auth.db.pool, workspace_id, user_id, session_id)
        .await
        .map_err(internal)?
        .map_err(map_db_error)?;
    Ok(Json(WebhookListResponse {
        items: rows.into_iter().map(webhook_output).collect(),
    }))
}

fn new_webhook_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

async fn create_webhook_route(
    State(state): State<AppState>,
    Extension(integrations): Extension<Arc<Integrations>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<WebhookCreateBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (user_id, session_id) = auth(
        &state,
        &headers,
        &jar,
        ApiTokenScope::WorkspaceManage,
        workspace_id,
    )
    .await?;
    let url = parse_target_url(&body.url, integrations.outbound.policy())
        .map_err(|_| AppError::with_source(ProblemCode::InvalidInput, "/url"))?;
    if body.events.iter().any(|e| e.is_empty() || e.len() > 100) {
        return Err(AppError::with_source(ProblemCode::InvalidInput, "/events"));
    }
    let events = normalize_events(&body.events)
        .ok_or_else(|| AppError::with_source(ProblemCode::InvalidInput, "/events"))?;
    let Some(keys) = integrations.encryption_keys.as_deref() else {
        tracing::warn!("webhook create refused: ENCRYPTION_KEYS is not configured");
        return Err(AppError::from_code(ProblemCode::IntegrationUnavailable));
    };
    let id = Uuid::now_v7();
    let secret = new_webhook_secret();
    let sealed = secret_box::seal(keys, &secret, &webhook_secret_context(workspace_id, id))
        .map_err(|_| AppError::internal())?;
    let ip = peer_ip(peer.ip());
    let row = create_webhook(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        NewWebhook {
            id,
            url: url.as_str(),
            events: &events,
            sealed_secret: &sealed,
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?
    .map_err(map_db_error)?;
    let out = webhook_output(row);
    Ok((
        StatusCode::CREATED,
        Json(WebhookCreatedOutput {
            id: out.id,
            workspace_id: out.workspace_id,
            url: out.url,
            events: out.events,
            created_at: out.created_at,
            updated_at: out.updated_at,
            secret,
        }),
    )
        .into_response())
}

async fn remove_webhook_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, webhook_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let (user_id, session_id) = auth(
        &state,
        &headers,
        &jar,
        ApiTokenScope::WorkspaceManage,
        workspace_id,
    )
    .await?;
    let ip = peer_ip(peer.ip());
    remove_webhook(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        webhook_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?
    .map_err(map_db_error)?;
    Ok(Json(OkResponse { ok: true }))
}

async fn get_github_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<GithubInstallOutput>, AppError> {
    let (user_id, session_id) = auth(
        &state,
        &headers,
        &jar,
        ApiTokenScope::WorkspaceManage,
        workspace_id,
    )
    .await?;
    let installation_id =
        github::get_installation(&state.auth.db.pool, workspace_id, user_id, session_id)
            .await
            .map_err(internal)?
            .map_err(map_db_error)?;
    Ok(Json(GithubInstallOutput { installation_id }))
}

async fn install_github_route(
    State(state): State<AppState>,
    Extension(integrations): Extension<Arc<Integrations>>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<GithubInstallUrlOutput>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let (user_id, session_id) = auth(
        &state,
        &headers,
        &jar,
        ApiTokenScope::WorkspaceManage,
        workspace_id,
    )
    .await?;
    // Source: without the app config the install is invalid input (400).
    let Some(config) = integrations.github.as_ref() else {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    };
    if !github::check_manager(&state.auth.db.pool, workspace_id, user_id, session_id)
        .await
        .map_err(internal)?
    {
        return Err(AppError::from_code(ProblemCode::NotFound));
    }
    let slug = fetch_app_slug(config).await.map_err(|err| {
        tracing::warn!(error = %err, "github.app_slug_failed");
        AppError::from_code(ProblemCode::InvalidInput)
    })?;
    // The state is single use and bound to this admin session (the callback
    // must come back with the same session cookie).
    let nonce = github::new_install_nonce();
    github::begin_install(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        &nonce,
    )
    .await
    .map_err(internal)?
    .map_err(map_db_error)?;
    let state_token =
        config.sign_install_state(workspace_id, &nonce, chrono::Utc::now().timestamp_millis());
    Ok(Json(GithubInstallUrlOutput {
        url: config.install_redirect(&slug, &state_token),
    }))
}

async fn remove_github_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let (user_id, session_id) = auth(
        &state,
        &headers,
        &jar,
        ApiTokenScope::WorkspaceManage,
        workspace_id,
    )
    .await?;
    let ip = peer_ip(peer.ip());
    github::remove_installation(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?
    .map_err(map_db_error)?;
    Ok(Json(OkResponse { ok: true }))
}

async fn link_issue_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<GithubIssueLinkBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (user_id, session_id) = auth(
        &state,
        &headers,
        &jar,
        ApiTokenScope::WorkspaceManage,
        workspace_id,
    )
    .await?;
    let Ok(issue_number) = i32::try_from(body.issue_number) else {
        return Err(AppError::with_source(
            ProblemCode::InvalidInput,
            "/issueNumber",
        ));
    };
    let linked = github::link_issue(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        body.task_id,
        &body.repo,
        issue_number,
    )
    .await
    .map_err(internal)?
    .map_err(map_db_error)?;
    Ok((
        StatusCode::CREATED,
        Json(GithubIssueLinkOutput {
            id: linked.id.to_string(),
            task_id: linked.task_id.to_string(),
            repo: linked.repo,
            issue_number: linked.issue_number,
        }),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    state: Option<String>,
    installation_id: Option<String>,
}

/// Source `completeGithubInstall`, hardened: the browser must bring back the
/// session that started the install, the nonce is consumed once, GitHub must
/// confirm the installation belongs to this app, and an existing link to
/// another installation is not overwritten.
async fn github_callback_route(
    State(state): State<AppState>,
    Extension(integrations): Extension<Arc<Integrations>>,
    headers: HeaderMap,
    jar: CookieJar,
    query: Result<Query<CallbackQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, AppError> {
    let invalid = || AppError::from_code(ProblemCode::InvalidInput);
    let Query(query) = query.map_err(|_| invalid())?;
    let Some(config) = integrations.github.as_ref() else {
        return Err(invalid());
    };
    let (Some(state_token), Some(installation_id)) = (query.state, query.installation_id) else {
        return Err(invalid());
    };
    let installation_id = installation_id.trim();
    if !github::installation_id_is_valid(installation_id) {
        return Err(invalid());
    }
    let Some((workspace_id, nonce)) =
        config.verify_install_state(&state_token, chrono::Utc::now().timestamp_millis())
    else {
        return Err(invalid());
    };
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Session, Some(workspace_id)).await?;
    match github::installation_exists(config, installation_id).await {
        Ok(true) => {}
        Ok(false) => return Err(invalid()),
        Err(err) => {
            tracing::warn!(error = %err, "github.installation_check_failed");
            return Err(invalid());
        }
    }
    github::complete_install(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        &nonce,
        installation_id,
    )
    .await
    .map_err(internal)?
    .map_err(|_| invalid())?;
    let target = format!("{}/", state.public_origin.trim_end_matches('/'));
    // Source answers 302 Found.
    Ok((StatusCode::FOUND, [(axum::http::header::LOCATION, target)]).into_response())
}

async fn github_webhook_route(
    State(state): State<AppState>,
    Extension(integrations): Extension<Arc<Integrations>>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<OkResponse>, AppError> {
    let Some(config) = integrations.github.as_ref() else {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    };
    let body = body.map_err(|rejection| {
        AppError::problem(
            if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                StatusCode::BAD_REQUEST
            },
            ProblemCode::InvalidInput,
        )
    })?;
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    match github::handle_webhook(
        &state.auth.db.pool,
        config,
        &body,
        header("x-hub-signature-256"),
        header("x-github-event"),
        header("x-github-delivery"),
    )
    .await
    {
        Ok(_) => Ok(Json(OkResponse { ok: true })),
        Err(InboundError::Signature) => {
            Err(AppError::from_code(ProblemCode::AuthenticationRequired))
        }
        Err(InboundError::Invalid) => Err(AppError::from_code(ProblemCode::InvalidInput)),
        Err(InboundError::Db(err)) => Err(internal(err)),
    }
}

/// Source `admit`: rate limit, then membership (404 hides whether AI is on),
/// then 503 when AI is off. Returns the document markdown source.
async fn ai_admit(
    state: &AppState,
    integrations: &Integrations,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
) -> Result<(Uuid, Uuid), AppError> {
    let (user_id, session_id) = auth(
        state,
        headers,
        jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
    )
    .await?;
    if let Err(retry_after) = state
        .rate_limiter
        .allow_window(&format!("ai-user:{user_id}"), 10, Duration::from_secs(300))
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    if !ai::is_member(&state.auth.db.pool, workspace_id, user_id, session_id)
        .await
        .map_err(internal)?
    {
        return Err(AppError::from_code(ProblemCode::NotFound));
    }
    let Some(config) = integrations.ai.as_ref() else {
        return Err(AppError::from_code(ProblemCode::AiUnavailable));
    };
    if state.document_convert.is_none() {
        tracing::warn!("ai request refused: FVOCI_DOCUMENT_CONVERT_BIN is unset");
        return Err(AppError::from_code(ProblemCode::AiUnavailable));
    }
    tracing::info!(key_hash = %config.key_hash(), "ai.request");
    Ok((user_id, session_id))
}

async fn ai_document(
    state: &AppState,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<(String, String), AppError> {
    let (title, content_json) = ai::viewable_document(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        document_id,
    )
    .await
    .map_err(internal)?
    .ok_or_else(|| AppError::from_code(ProblemCode::NotFound))?;
    let convert = state
        .document_convert
        .as_ref()
        .ok_or_else(|| AppError::from_code(ProblemCode::AiUnavailable))?;
    // An empty title makes the helper's markdown export the bare body, which
    // is source `documentContentMd`.
    let (bytes, _, _) = convert
        .export_binary("export_md", "", &content_json)
        .await
        .map_err(|err| {
            tracing::warn!(error = %err, "ai.markdown_failed");
            AppError::internal()
        })?;
    let markdown = String::from_utf8(bytes).map_err(|_| AppError::internal())?;
    Ok((markdown, title))
}

async fn ai_summarize_route(
    State(state): State<AppState>,
    Extension(integrations): Extension<Arc<Integrations>>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<AiDocumentBody>, JsonRejection>,
) -> Result<Json<AiSummarizeOutput>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (user_id, session_id) =
        ai_admit(&state, &integrations, &headers, &jar, workspace_id).await?;
    let (markdown, title) =
        ai_document(&state, workspace_id, user_id, session_id, body.document_id).await?;
    Ok(Json(AiSummarizeOutput {
        summary: ai::summarize(&markdown, &title),
    }))
}

async fn ai_generate_tasks_route(
    State(state): State<AppState>,
    Extension(integrations): Extension<Arc<Integrations>>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<AiDocumentBody>, JsonRejection>,
) -> Result<Json<AiGenerateTasksOutput>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (user_id, session_id) =
        ai_admit(&state, &integrations, &headers, &jar, workspace_id).await?;
    let (markdown, title) =
        ai_document(&state, workspace_id, user_id, session_id, body.document_id).await?;
    Ok(Json(AiGenerateTasksOutput {
        titles: ai::titles(&markdown, &title),
    }))
}

async fn ai_suggest_links_route(
    State(state): State<AppState>,
    Extension(integrations): Extension<Arc<Integrations>>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<AiDocumentBody>, JsonRejection>,
) -> Result<Json<AiSuggestLinksOutput>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (user_id, session_id) =
        ai_admit(&state, &integrations, &headers, &jar, workspace_id).await?;
    ai::viewable_document(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        body.document_id,
    )
    .await
    .map_err(internal)?
    .ok_or_else(|| AppError::from_code(ProblemCode::NotFound))?;
    let ids =
        ai::visible_document_ids(&state.auth.db.pool, workspace_id, user_id, body.document_id)
            .await
            .map_err(internal)?;
    Ok(Json(AiSuggestLinksOutput {
        document_ids: ids.into_iter().map(|id| id.to_string()).collect(),
    }))
}
