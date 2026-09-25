//! Account lifecycle routes.
//!
//! Source: apps/server/src/domains/identity/{consent,magic-link,oidc,export}.ts
//! and domains/workspaces/me.ts. Session-only routes reject API tokens (404),
//! authenticate before any rate-limit bucket is touched (source onParse only
//! charges authenticated callers), and recheck the session inside the write
//! transaction.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::api::dto::{
    DashboardProjectOutput, DashboardRecentItemOutput, DashboardWorkspaceOutput, EmailChangeBody,
    ErasureScheduleOutput, LabelOutput, MagicLinkBody, MeDashboardResponse, MeLocateResponse,
    MemberResponse, OkResponse, PasswordChangeBody, TaskListItemOutput, TokenBody, WithdrawBody,
    WorkspaceStatusOutput,
};
use crate::attachments::ObjectStorage;
use crate::auth::password::{hash_password, verify_password};
use crate::auth::token::new_token;
use crate::db::account::{self, CancelWithdrawOutcome, WithdrawConfirm, WithdrawError};
use crate::db::dashboard::{self, DashboardError, LocateKind};
use crate::db::identity::password_hash_by_id;
use crate::db::magic::{consume_magic_token, magic_expires_at};
use crate::db::user_export::{self, ExportAttachment, ExportProfile};
use crate::error::{AppError, ProblemCode};
use crate::export_zip::{zip_safe_name, ZipStream};
use crate::http::authz::{require_request_auth, Access, RequestAuth};
use crate::http::cookie::clear_session_cookie;
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::mail::{equalize_magic_response_timing, templates, MAGIC_PER_EMAIL, MAGIC_PER_IP};
use crate::validate::{normalize_email, validate_password_length};

/// Source `http-rate-limit.ts` named limits (5-minute window unless noted).
const WITHDRAW_PER_IP: u32 = 30;
const WITHDRAW_PER_ACCOUNT: u32 = 10;
const CANCEL_WITHDRAW_PER_IP: u32 = 30;
const PASSWORD_CHANGE_PER_IP: u32 = 30;
const PASSWORD_CHANGE_PER_ACCOUNT: u32 = 10;
const USER_EXPORT_PER_USER: u32 = 5;
const USER_EXPORT_WINDOW: Duration = Duration::from_secs(15 * 60);

/// Chunks in flight between the export producer and the response body.
const EXPORT_CHANNEL_DEPTH: usize = 4;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/withdraw", post(withdraw))
        .route("/api/v1/auth/cancel-withdraw", post(cancel_withdraw))
        .route("/api/v1/auth/email", patch(request_email_change))
        .route("/api/v1/auth/email/confirm", post(confirm_email_change))
        .route("/api/v1/auth/password", patch(change_password))
        .route("/api/v1/auth/magic-link", post(request_magic_link))
        .route("/api/v1/auth/magic-link/consume", post(consume_magic_link))
        .route("/api/v1/me/export", get(export))
        .route("/api/v1/me/dashboard", get(me_dashboard))
        .route("/api/v1/me/locate", get(me_locate))
}

async fn limit(state: &AppState, key: String, max: u32) -> Result<(), AppError> {
    state
        .rate_limiter
        .allow(&key, max)
        .await
        .map_err(AppError::rate_limited)
}

async fn session(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
) -> Result<RequestAuth, AppError> {
    require_request_auth(state, headers, jar, Access::Session, None).await
}

fn non_empty(value: String) -> Result<String, AppError> {
    if value.is_empty() {
        return Err(AppError::with_source(ProblemCode::InvalidInput, "/"));
    }
    Ok(value)
}

fn internal(err: sqlx::Error) -> AppError {
    let message = match &err {
        sqlx::Error::Database(db) => db.message().to_string(),
        _ => "database operation failed".to_string(),
    };
    tracing::error!("database error: {message}");
    AppError::internal()
}

fn with_cookie(mut response: Response, cookie: String) -> Response {
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

fn link_url(state: &AppState, path: &str, token: &str) -> String {
    format!(
        "{}{path}?token={token}",
        state.public_origin.trim_end_matches('/')
    )
}

async fn withdraw(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<WithdrawBody>, JsonRejection>,
) -> Result<Response, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = session(&state, &headers, &jar).await?;
    let ip = peer_ip(peer.ip());
    limit(&state, format!("withdraw-ip:{ip}"), WITHDRAW_PER_IP).await?;
    limit(
        &state,
        format!("withdraw-account:{ip}:{}", auth.user_id),
        WITHDRAW_PER_ACCOUNT,
    )
    .await?;
    let Json(body) = body.map_err(AppError::from)?;
    let pool = &state.auth.db.pool;
    let confirm = if let Some(password) = body.current_password {
        let password = non_empty(password)?;
        let Some(stored) = password_hash_by_id(pool, auth.user_id)
            .await
            .map_err(internal)?
        else {
            return Err(AppError::from_code(ProblemCode::ConfirmInvalid));
        };
        if !verify_password(Some(&stored), &password, &state.auth.password_keys)
            .await
            .ok
        {
            return Err(AppError::from_code(ProblemCode::ConfirmInvalid));
        }
        WithdrawConfirm::Password {
            verified_hash: stored,
        }
    } else if let Some(local) = body.email_local_part {
        WithdrawConfirm::EmailLocalPart(non_empty(local)?)
    } else {
        return Err(AppError::from_code(ProblemCode::ConfirmInvalid));
    };
    let scheduled =
        account::withdraw_user(pool, auth.user_id, auth.credential_id, &confirm, Some(&ip))
            .await
            .map_err(internal)?
            .map_err(|err| match err {
                WithdrawError::ConfirmInvalid => AppError::from_code(ProblemCode::ConfirmInvalid),
                WithdrawError::OwnerTransferRequired => {
                    AppError::from_code(ProblemCode::OwnerTransferRequired)
                }
                WithdrawError::LastInstanceAdmin => {
                    AppError::from_code(ProblemCode::LastInstanceAdmin)
                }
                WithdrawError::SessionGone => {
                    AppError::from_code(ProblemCode::AuthenticationRequired)
                }
            })?;
    // Source sendErasureCancelMail: awaited; a failure only turns mailSent off
    // because the cancel token already committed.
    let mail_sent = if state.mailer.enabled() {
        let url = format!(
            "{}/cancel-withdraw#token={}",
            state.public_origin.trim_end_matches('/'),
            scheduled.cancel_token
        );
        match state
            .mailer
            .send(
                &scheduled.email,
                templates::WITHDRAW_CANCEL_SUBJECT,
                &templates::withdraw_cancel_text(&url),
            )
            .await
        {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(message = %err, "mail.send_failed");
                false
            }
        }
    } else {
        false
    };
    let response = Json(ErasureScheduleOutput {
        ok: true,
        cancel_token: scheduled.cancel_token,
        erase_at: scheduled.erase_at,
        mail_sent,
    })
    .into_response();
    Ok(with_cookie(
        response,
        clear_session_cookie(state.cookie_secure),
    ))
}

async fn cancel_withdraw(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<TokenBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let ip = peer_ip(peer.ip());
    limit(
        &state,
        format!("cancel-withdraw-ip:{ip}"),
        CANCEL_WITHDRAW_PER_IP,
    )
    .await?;
    let Json(body) = body.map_err(AppError::from)?;
    let token = non_empty(body.token)?;
    match account::cancel_withdraw(&state.auth.db.pool, &token, Some(&ip))
        .await
        .map_err(internal)?
    {
        CancelWithdrawOutcome::Ok => Ok(Json(OkResponse { ok: true })),
        CancelWithdrawOutcome::NotFound | CancelWithdrawOutcome::DeadlinePassed => {
            Err(AppError::from_code(ProblemCode::NotFound))
        }
    }
}

async fn request_email_change(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<EmailChangeBody>, JsonRejection>,
) -> Result<(StatusCode, Json<OkResponse>), AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = session(&state, &headers, &jar).await?;
    let ip = peer_ip(peer.ip());
    limit(&state, format!("magic-ip:{ip}"), MAGIC_PER_IP).await?;
    let Json(body) = body.map_err(AppError::from)?;
    let new_email = normalize_email(&body.new_email)?;
    limit(
        &state,
        format!("magic-email:{ip}:{new_email}"),
        MAGIC_PER_EMAIL,
    )
    .await?;
    let started = Instant::now();
    let pool = &state.auth.db.pool;
    if let Some(user) = account::live_user_identity(pool, auth.user_id)
        .await
        .map_err(internal)?
    {
        // Source requestEmailChange: an address already on any row (withdrawn
        // included) gets no mail, and the response does not say so.
        if !account::email_exists(pool, &new_email)
            .await
            .map_err(internal)?
        {
            let issued = new_token();
            account::issue_email_change_token(
                pool,
                auth.user_id,
                user.generation,
                &new_email,
                &issued.hash,
                magic_expires_at(Utc::now()),
            )
            .await
            .map_err(internal)?;
            let url = link_url(&state, "/confirm-email", &issued.token);
            state.mailer.send_detached(
                new_email,
                templates::EMAIL_CHANGE_SUBJECT.to_string(),
                templates::magic_link_text(&url),
            );
            state.mailer.send_detached(
                user.email,
                templates::EMAIL_CHANGE_REQUESTED_SUBJECT.to_string(),
                templates::EMAIL_CHANGE_REQUESTED_TEXT.to_string(),
            );
        }
    }
    equalize_magic_response_timing(started).await;
    Ok((StatusCode::ACCEPTED, Json(OkResponse { ok: true })))
}

async fn confirm_email_change(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<TokenBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let ip = peer_ip(peer.ip());
    limit(&state, format!("magic-ip:{ip}"), MAGIC_PER_IP).await?;
    let Json(body) = body.map_err(AppError::from)?;
    let token = non_empty(body.token)?;
    let pool = &state.auth.db.pool;
    let Some(payload) = consume_magic_token(pool, &token).await.map_err(internal)? else {
        return Err(AppError::from_code(ProblemCode::MagicInvalid));
    };
    let Some(changed) = account::complete_email_change(pool, &payload, Some(&ip))
        .await
        .map_err(internal)?
    else {
        return Err(AppError::from_code(ProblemCode::MagicInvalid));
    };
    state.mailer.send_detached(
        changed.old_email,
        templates::EMAIL_CHANGE_COMPLETED_SUBJECT.to_string(),
        templates::EMAIL_CHANGE_COMPLETED_TEXT.to_string(),
    );
    Ok(Json(OkResponse { ok: true }))
}

async fn change_password(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<PasswordChangeBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = session(&state, &headers, &jar).await?;
    let ip = peer_ip(peer.ip());
    limit(
        &state,
        format!("password-change-ip:{ip}"),
        PASSWORD_CHANGE_PER_IP,
    )
    .await?;
    limit(
        &state,
        format!("password-change-account:{ip}:{}", auth.user_id),
        PASSWORD_CHANGE_PER_ACCOUNT,
    )
    .await?;
    let Json(body) = body.map_err(AppError::from)?;
    let current = body.current_password.map(non_empty).transpose()?;
    validate_password_length(&body.new_password)?;
    let pool = &state.auth.db.pool;
    let stored = password_hash_by_id(pool, auth.user_id)
        .await
        .map_err(internal)?;
    if let Some(stored) = stored.as_deref() {
        let Some(current) = current.as_deref() else {
            return Err(AppError::from_code(ProblemCode::PasswordInvalid));
        };
        if !verify_password(Some(stored), current, &state.auth.password_keys)
            .await
            .ok
        {
            return Err(AppError::from_code(ProblemCode::PasswordInvalid));
        }
    }
    let new_hash = hash_password(&body.new_password, &state.auth.password_keys)
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "password hash failed");
            AppError::internal()
        })?;
    let changed = account::change_password(
        pool,
        auth.user_id,
        auth.credential_id,
        stored.as_deref(),
        &new_hash,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    if !changed {
        return Err(AppError::from_code(ProblemCode::PasswordInvalid));
    }
    Ok(Json(OkResponse { ok: true }))
}

async fn request_magic_link(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<MagicLinkBody>, JsonRejection>,
) -> Result<(StatusCode, Json<OkResponse>), AppError> {
    check_origin(&headers, &state.public_origin)?;
    let ip = peer_ip(peer.ip());
    limit(&state, format!("magic-ip:{ip}"), MAGIC_PER_IP).await?;
    let Json(body) = body.map_err(AppError::from)?;
    let email = normalize_email(&body.email)?;
    limit(&state, format!("magic-email:{ip}:{email}"), MAGIC_PER_EMAIL).await?;
    let started = Instant::now();
    let pool = &state.auth.db.pool;
    if let Some(user) = account::login_link_user(pool, &email)
        .await
        .map_err(internal)?
    {
        let issued = new_token();
        account::issue_login_token(pool, &user, &issued.hash, magic_expires_at(Utc::now()))
            .await
            .map_err(internal)?;
        let url = link_url(&state, "/magic-link", &issued.token);
        state.mailer.send_detached(
            email,
            templates::MAGIC_LOGIN_SUBJECT.to_string(),
            templates::magic_link_text(&url),
        );
    }
    equalize_magic_response_timing(started).await;
    Ok((StatusCode::ACCEPTED, Json(OkResponse { ok: true })))
}

async fn consume_magic_link(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<TokenBody>, JsonRejection>,
) -> Result<Response, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let ip = peer_ip(peer.ip());
    limit(&state, format!("magic-ip:{ip}"), MAGIC_PER_IP).await?;
    let Json(body) = body.map_err(AppError::from)?;
    let token = non_empty(body.token)?;
    let pool = &state.auth.db.pool;
    let Some(payload) = consume_magic_token(pool, &token).await.map_err(internal)? else {
        return Err(AppError::from_code(ProblemCode::MagicInvalid));
    };
    let Some(issued) = account::complete_magic_login(pool, &payload)
        .await
        .map_err(internal)?
    else {
        return Err(AppError::from_code(ProblemCode::MagicInvalid));
    };
    Ok(crate::http::routes::auth::issued_response(
        state.cookie_secure,
        issued,
    ))
}

fn strict_query(
    raw: Option<String>,
    allowed: &[&str],
) -> Result<HashMap<String, String>, AppError> {
    let mut out = HashMap::new();
    let raw = raw.unwrap_or_default();
    for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
        if !allowed.contains(&key.as_ref()) || out.contains_key(key.as_ref()) {
            return Err(AppError::with_source(ProblemCode::InvalidInput, "/"));
        }
        out.insert(key.into_owned(), value.into_owned());
    }
    Ok(out)
}

fn parse_uuid_param(value: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(value).map_err(|_| AppError::with_source(ProblemCode::InvalidInput, "/"))
}

async fn me_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    RawQuery(raw): RawQuery,
) -> Result<Json<MeDashboardResponse>, AppError> {
    let auth = session(&state, &headers, &jar).await?;
    let query = strict_query(raw, &["lastVisited"])?;
    let last_visited = query
        .get("lastVisited")
        .map(|value| parse_uuid_param(value))
        .transpose()?;
    let board = dashboard::build_dashboard(
        &state.auth.db.pool,
        auth.user_id,
        auth.credential_id,
        last_visited,
    )
    .await
    .map_err(internal)?
    .map_err(|DashboardError::SessionGone| {
        AppError::from_code(ProblemCode::AuthenticationRequired)
    })?;
    Ok(Json(MeDashboardResponse {
        assigned: board
            .assigned
            .into_iter()
            .map(|row| TaskListItemOutput {
                assignee_ids: row.assignee_ids.iter().map(ToString::to_string).collect(),
                label_ids: row.label_ids.iter().map(ToString::to_string).collect(),
                meta: crate::http::routes::tasks::task_meta_output(row.meta),
            })
            .collect(),
        recent: board
            .recent
            .into_iter()
            .map(|item| DashboardRecentItemOutput {
                kind: item.kind.to_string(),
                id: item.id.to_string(),
                title: item.title,
                project_id: item.project_id.map(|id| id.to_string()),
                number: item.number,
                updated_at: item.updated_at,
                workspace_id: item.workspace_id.to_string(),
            })
            .collect(),
        labels: board
            .labels
            .into_iter()
            .map(|label| LabelOutput {
                id: label.id.to_string(),
                project_id: label.project_id.to_string(),
                name: label.name,
                color: label.color,
            })
            .collect(),
        statuses: board
            .statuses
            .into_iter()
            .map(|status| WorkspaceStatusOutput {
                id: status.id.to_string(),
                workflow_id: status.workflow_id.to_string(),
                project_id: status.project_id.to_string(),
                name: status.name,
                sort_key: status.sort_key,
                category: status.category,
                wip_limit: status.wip_limit,
            })
            .collect(),
        projects: board
            .projects
            .into_iter()
            .map(|project| DashboardProjectOutput {
                id: project.id.to_string(),
                workspace_id: project.workspace_id.to_string(),
                key: project.key,
                name: project.name,
            })
            .collect(),
        members: board
            .members
            .into_iter()
            .map(|member| MemberResponse {
                user_id: member.user_id.to_string(),
                email: member.email,
                given_name: member.given_name,
                family_name: member.family_name,
                role: member.role.as_str().to_string(),
            })
            .collect(),
        workspaces: board
            .workspaces
            .into_iter()
            .map(|ws| DashboardWorkspaceOutput {
                id: ws.id.to_string(),
                name: ws.name,
                slug: ws.slug,
                role: ws.role.as_str().to_string(),
                kind: ws.kind,
                document_count: ws.document_count,
                assigned_count: ws.assigned_count,
                unread_count: ws.unread_count,
            })
            .collect(),
        unread_count: board.unread_count,
    }))
}

async fn me_locate(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    RawQuery(raw): RawQuery,
) -> Result<Json<MeLocateResponse>, AppError> {
    let auth = session(&state, &headers, &jar).await?;
    let query = strict_query(raw, &["type", "id"])?;
    let kind = match query.get("type").map(String::as_str) {
        Some("task") => LocateKind::Task,
        Some("document") => LocateKind::Document,
        _ => return Err(AppError::with_source(ProblemCode::InvalidInput, "/type")),
    };
    let Some(id) = query.get("id") else {
        return Err(AppError::with_source(ProblemCode::InvalidInput, "/id"));
    };
    let id = parse_uuid_param(id)?;
    let found = dashboard::locate_for_user(
        &state.auth.db.pool,
        auth.user_id,
        auth.credential_id,
        kind,
        id,
    )
    .await
    .map_err(internal)?
    .map_err(|DashboardError::SessionGone| {
        AppError::from_code(ProblemCode::AuthenticationRequired)
    })?;
    match found {
        Some(workspace_id) => Ok(Json(MeLocateResponse {
            workspace_id: workspace_id.to_string(),
        })),
        None => Err(AppError::from_code(ProblemCode::NotFound)),
    }
}

// ---------------------------------------------------------------------------
// GET /me/export — source buildUserExportZip, streamed.

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileJson<'a> {
    id: String,
    email: &'a str,
    given_name: &'a str,
    family_name: Option<&'a str>,
    locale: &'a str,
    timezone: &'a str,
    week_starts_on: i32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommentJson<'a> {
    id: String,
    workspace_id: String,
    document_id: Option<String>,
    task_id: Option<String>,
    body: &'a str,
    created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AttachmentJson<'a> {
    id: String,
    workspace_id: String,
    name: &'a str,
    mime: &'a str,
    size_bytes: Option<i64>,
    scan_status: &'a str,
}

/// JS `toISOString()`: millisecond precision, `Z`.
fn js_iso(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// JS `JSON.stringify(value, null, 2)` for one array element, indented by 2.
fn pretty_element<T: Serialize>(value: &T) -> Result<String, ExportFailure> {
    let text = serde_json::to_string_pretty(value).map_err(|_| ExportFailure::Encode)?;
    Ok(text
        .lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n"))
}

#[derive(Debug)]
enum ExportFailure {
    /// The client went away; stop quietly.
    Closed,
    Db,
    Storage,
    Zip,
    Encode,
}

type ExportSender = mpsc::Sender<Result<Bytes, io::Error>>;

struct ExportWriter {
    tx: ExportSender,
    zip: ZipStream,
}

impl ExportWriter {
    async fn send(&self, bytes: Bytes) -> Result<(), ExportFailure> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.tx
            .send(Ok(bytes))
            .await
            .map_err(|_| ExportFailure::Closed)
    }

    async fn begin(&mut self, name: &str) -> Result<(), ExportFailure> {
        let header = self.zip.begin_entry(name).map_err(|_| ExportFailure::Zip)?;
        self.send(header).await
    }

    async fn data(&mut self, bytes: Bytes) -> Result<(), ExportFailure> {
        self.zip
            .entry_data(&bytes)
            .map_err(|_| ExportFailure::Zip)?;
        self.send(bytes).await
    }

    async fn end(&mut self) -> Result<(), ExportFailure> {
        let descriptor = self.zip.end_entry().map_err(|_| ExportFailure::Zip)?;
        self.send(descriptor).await
    }

    /// Small entry already in memory: sizes in the local header.
    async fn file(&mut self, name: &str, bytes: Bytes) -> Result<(), ExportFailure> {
        let chunk = self
            .zip
            .whole_entry(name, &bytes)
            .map_err(|_| ExportFailure::Zip)?;
        self.send(chunk).await
    }
}

async fn write_export(
    writer: &mut ExportWriter,
    pool: &sqlx::PgPool,
    storage: &ObjectStorage,
    user_id: Uuid,
    profile: ExportProfile,
    workspaces: Vec<Uuid>,
    attachments: Vec<ExportAttachment>,
) -> Result<(), ExportFailure> {
    let profile_json = serde_json::to_string_pretty(&ProfileJson {
        id: profile.id.to_string(),
        email: &profile.email,
        given_name: &profile.given_name,
        family_name: profile.family_name.as_deref(),
        locale: &profile.locale,
        timezone: &profile.timezone,
        week_starts_on: profile.week_starts_on,
    })
    .map_err(|_| ExportFailure::Encode)?;
    writer
        .file("profile.json", Bytes::from(format!("{profile_json}\n")))
        .await?;

    writer.begin("comments.json").await?;
    let mut first = true;
    for workspace_id in workspaces {
        let mut after = None;
        loop {
            let page = user_export::export_comment_page(pool, workspace_id, user_id, after)
                .await
                .map_err(|err| {
                    tracing::error!(error = %err, "user_export.comments_failed");
                    ExportFailure::Db
                })?;
            let Some(last) = page.last() else {
                break;
            };
            after = Some((last.created_at, last.id));
            let full = page.len() as i64 == user_export::EXPORT_COMMENT_PAGE;
            let mut chunk = String::new();
            for comment in &page {
                chunk.push_str(if first { "[\n" } else { ",\n" });
                first = false;
                chunk.push_str(&pretty_element(&CommentJson {
                    id: comment.id.to_string(),
                    workspace_id: comment.workspace_id.to_string(),
                    document_id: comment.document_id.map(|id| id.to_string()),
                    task_id: comment.task_id.map(|id| id.to_string()),
                    body: &comment.body,
                    created_at: js_iso(comment.created_at),
                })?);
            }
            writer.data(Bytes::from(chunk)).await?;
            if !full {
                break;
            }
        }
    }
    writer
        .data(Bytes::from_static(if first { b"[]\n" } else { b"\n]\n" }))
        .await?;
    writer.end().await?;

    let mut listing = String::new();
    for (index, row) in attachments.iter().enumerate() {
        listing.push_str(if index == 0 { "[\n" } else { ",\n" });
        listing.push_str(&pretty_element(&AttachmentJson {
            id: row.id.to_string(),
            workspace_id: row.workspace_id.to_string(),
            name: &row.name,
            mime: &row.mime,
            size_bytes: row.size_bytes,
            scan_status: &row.scan_status,
        })?);
    }
    listing.push_str(if attachments.is_empty() {
        "[]\n"
    } else {
        "\n]\n"
    });
    writer
        .file("attachments.json", Bytes::from(listing))
        .await?;

    for row in &attachments {
        if row.scan_status == "infected" {
            continue;
        }
        // Source packAttachmentFiles skips a missing object (ENOENT).
        let Some(size) = storage.head(&row.storage_key).await.map_err(|err| {
            tracing::error!(error = %err, "user_export.storage_head_failed");
            ExportFailure::Storage
        })?
        else {
            continue;
        };
        let name = format!("attachments/{}-{}", row.id, zip_safe_name(&row.name));
        if size == 0 {
            writer.file(&name, Bytes::new()).await?;
            continue;
        }
        let mut stream = match storage
            .open_payload_stream(&row.storage_key, 0, size - 1)
            .await
        {
            Ok(stream) => stream,
            Err(err) => {
                if storage
                    .head(&row.storage_key)
                    .await
                    .ok()
                    .flatten()
                    .is_none()
                {
                    continue;
                }
                tracing::error!(error = %err, "user_export.storage_open_failed");
                return Err(ExportFailure::Storage);
            }
        };
        writer.begin(&name).await?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| {
                tracing::error!(error = %err, "user_export.storage_read_failed");
                ExportFailure::Storage
            })?;
            writer.data(chunk).await?;
        }
        writer.end().await?;
    }
    let tail = std::mem::take(&mut writer.zip)
        .finish()
        .map_err(|_| ExportFailure::Zip)?;
    writer.send(tail).await
}

async fn export(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let auth = session(&state, &headers, &jar).await?;
    state
        .rate_limiter
        .allow_window(
            &format!("user-export:{}", auth.user_id),
            USER_EXPORT_PER_USER,
            USER_EXPORT_WINDOW,
        )
        .await
        .map_err(AppError::rate_limited)?;
    let pool = state.auth.db.pool.clone();
    let Some(profile) = user_export::export_profile(&pool, auth.user_id)
        .await
        .map_err(internal)?
    else {
        return Err(AppError::from_code(ProblemCode::NotFound));
    };
    let workspaces = user_export::export_membership_workspaces(&pool, auth.user_id)
        .await
        .map_err(internal)?;
    let attachments = user_export::export_attachments(&pool, auth.user_id)
        .await
        .map_err(internal)?;
    // Zip32 like the source: refuse up front rather than cut the stream.
    let declared: u64 = attachments
        .iter()
        .filter(|row| row.scan_status != "infected")
        .map(|row| row.size_bytes.unwrap_or(0).max(0) as u64)
        .sum();
    if declared > u64::from(u32::MAX) - 64 * 1024 * 1024 || attachments.len() > 60_000 {
        tracing::error!(
            attachments = attachments.len(),
            "user_export.too_large_for_zip32"
        );
        return Err(AppError::internal());
    }

    let (tx, rx) = mpsc::channel(EXPORT_CHANNEL_DEPTH);
    let storage = state.storage.clone();
    let user_id = auth.user_id;
    tokio::spawn(async move {
        let mut writer = ExportWriter {
            tx,
            zip: ZipStream::new(),
        };
        match write_export(
            &mut writer,
            &pool,
            &storage,
            user_id,
            profile,
            workspaces,
            attachments,
        )
        .await
        {
            Ok(()) | Err(ExportFailure::Closed) => {}
            Err(failure) => {
                tracing::error!(?failure, "user_export.failed");
                // Abort the body so the client sees a failed download, not a
                // truncated archive that looks complete.
                let _ = writer
                    .tx
                    .send(Err(io::Error::other("user export failed")))
                    .await;
            }
        }
    });
    let body = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"fvoci-export.zip\"",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(body),
    )
        .into_response())
}
