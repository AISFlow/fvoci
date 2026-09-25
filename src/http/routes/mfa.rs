//! TOTP MFA routes (source `apps/server/src/domains/identity/mfa.ts`).
//!
//! All but `/auth/mfa/verify` are session only (API tokens get 404). Setup and
//! disable re-authenticate; setup, enable, disable and verify are limited per
//! account (5 / 5 min) because each is a guessing surface.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use axum_extra::extract::CookieJar;
use chrono::Utc;
use uuid::Uuid;

use crate::api::dto::{
    MfaDisableBody, MfaEnableBody, MfaSetupBody, MfaSetupOutput, MfaStatusOutput, MfaVerifyBody,
    OkResponse, SessionIssuedOutput,
};
use crate::auth::password::verify_password;
use crate::auth::token::hash_token;
use crate::auth::totp;
use crate::db::identity::password_hash_by_id;
use crate::db::mfa::{
    self, ChallengeCheck, DisableOutcome, EnableOutcome, MfaRow, SecondFactor, SetupOutcome,
};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access, RequestAuth};
use crate::http::cookie::set_session_cookie;
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::identity::{user_mfa_context, Identity};
use crate::secret_box;

/// Source `http-rate-limit.ts` LIMITS (5-minute window).
const MFA_VERIFY_PER_IP: u32 = 30;
const MFA_VERIFY_PER_USER: u32 = 5;
const MFA_VERIFY_WINDOW_SECS: u32 = 300;
const MFA_REAUTH_PER_USER: u32 = 5;
const MFA_ENABLE_PER_USER: u32 = 5;
/// A password-less account may set up MFA only from a session this fresh.
const FRESH_AUTH_SECS: i64 = 600;

pub fn router(identity: Arc<Identity>) -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/mfa", get(status))
        .route("/api/v1/auth/mfa/setup", post(setup))
        .route("/api/v1/auth/mfa/enable", post(enable))
        .route("/api/v1/auth/mfa/disable", post(disable))
        .route("/api/v1/auth/mfa/verify", post(verify))
        .layer(Extension(identity))
}

fn problem(code: ProblemCode) -> AppError {
    AppError::from_code(code)
}

fn internal(err: sqlx::Error) -> AppError {
    let message = match &err {
        sqlx::Error::Database(db) => db.message().to_string(),
        _ => "database operation failed".to_string(),
    };
    tracing::error!("database error: {message}");
    AppError::internal()
}

async fn limit(state: &AppState, key: String, max: u32) -> Result<(), AppError> {
    state
        .rate_limiter
        .allow(&key, max)
        .await
        .map_err(AppError::rate_limited)
}

/// Source `mfaCode`: trimmed, 6..=16 characters.
fn mfa_code(raw: &str) -> Result<String, AppError> {
    let code = raw.trim();
    let len = code.chars().count();
    if !(6..=16).contains(&len) {
        return Err(AppError::with_source(ProblemCode::InvalidInput, "/code"));
    }
    Ok(code.to_string())
}

fn optional_password(raw: Option<String>) -> Result<Option<String>, AppError> {
    match raw {
        Some(p) if p.is_empty() => Err(AppError::with_source(
            ProblemCode::InvalidInput,
            "/currentPassword",
        )),
        other => Ok(other),
    }
}

async fn session(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
) -> Result<RequestAuth, AppError> {
    require_request_auth(state, headers, jar, Access::Session, None).await
}

fn keys(identity: &Identity) -> Result<&crate::auth::password::Keyring, AppError> {
    identity.encryption_keys.as_deref().ok_or_else(|| {
        tracing::warn!("mfa refused: ENCRYPTION_KEYS is not configured");
        problem(ProblemCode::EncryptionUnavailable)
    })
}

fn open_secret(identity: &Identity, row: &MfaRow) -> Result<Vec<u8>, AppError> {
    let hex_secret = secret_box::open(
        keys(identity)?,
        &row.totp_secret,
        &user_mfa_context(row.user_id),
    )
    .map_err(|err| {
        tracing::error!(error = %err, "mfa secret does not open");
        AppError::internal()
    })?;
    hex::decode(hex_secret).map_err(|_| AppError::internal())
}

/// Source `verifyEnabledCode` without the claim: which factor the code is.
fn check_code(
    identity: &Identity,
    row: &MfaRow,
    code: &str,
) -> Result<Option<(SecondFactor, Option<String>)>, AppError> {
    if totp::is_totp_shape(code) {
        let secret = open_secret(identity, row)?;
        return Ok(totp::match_totp(&secret, code, identity.now_ms())
            .map(|step| (SecondFactor::Totp(step), None)));
    }
    let normalized = totp::normalize_recovery_code(code);
    if normalized.len() != totp::RECOVERY_CODE_LENGTH {
        return Ok(None);
    }
    Ok(Some((
        SecondFactor::Recovery,
        Some(hash_token(&normalized)),
    )))
}

/// `none` when the account has no password (source `confirmPassword`).
enum PasswordCheck {
    Ok,
    Invalid,
    None,
}

async fn confirm_password(
    state: &AppState,
    user_id: Uuid,
    current: Option<&str>,
) -> Result<PasswordCheck, AppError> {
    let Some(stored) = password_hash_by_id(&state.auth.db.pool, user_id)
        .await
        .map_err(internal)?
    else {
        return Ok(PasswordCheck::None);
    };
    let Some(current) = current else {
        return Ok(PasswordCheck::Invalid);
    };
    Ok(
        if verify_password(Some(&stored), current, &state.auth.password_keys)
            .await
            .ok
        {
            PasswordCheck::Ok
        } else {
            PasswordCheck::Invalid
        },
    )
}

async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<MfaStatusOutput>, AppError> {
    let auth = session(&state, &headers, &jar).await?;
    let row = mfa::find(&state.auth.db.pool, auth.user_id)
        .await
        .map_err(internal)?;
    let enabled = row.as_ref().is_some_and(|r| r.enabled_at.is_some());
    Ok(Json(MfaStatusOutput {
        enabled,
        recovery_codes_left: if enabled {
            row.map(|r| r.recovery_hashes.len() as i64).unwrap_or(0)
        } else {
            0
        },
    }))
}

async fn session_created_recently(state: &AppState, session_id: Uuid) -> Result<bool, AppError> {
    let created: Option<(chrono::DateTime<Utc>,)> =
        sqlx::query_as("SELECT created_at FROM fvoci.sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(&state.auth.db.pool)
            .await
            .map_err(internal)?;
    Ok(created.is_some_and(|(at,)| (Utc::now() - at).num_seconds() <= FRESH_AUTH_SECS))
}

async fn setup(
    State(state): State<AppState>,
    Extension(identity): Extension<Arc<Identity>>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<MfaSetupBody>, JsonRejection>,
) -> Result<Json<MfaSetupOutput>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = session(&state, &headers, &jar).await?;
    limit(
        &state,
        format!("mfa-reauth:{}", auth.user_id),
        MFA_REAUTH_PER_USER,
    )
    .await?;
    let Json(body) = body.map_err(AppError::from)?;
    let current = optional_password(body.current_password)?;
    // A password-less account has nothing to re-enter: only a session that
    // just finished its first factor may attach an authenticator, so a stolen
    // old session cannot lock the owner out.
    if !auth.user.has_password && !session_created_recently(&state, auth.credential_id).await? {
        return Err(problem(ProblemCode::MfaReauthRequired));
    }
    if let PasswordCheck::Invalid =
        confirm_password(&state, auth.user_id, current.as_deref()).await?
    {
        return Err(problem(ProblemCode::MfaPasswordInvalid));
    }
    let keyring = keys(&identity)?;
    let raw = totp::new_secret();
    let recovery = totp::new_recovery_codes();
    let hashes: Vec<String> = recovery.iter().map(|c| hash_token(c)).collect();
    let sealed = secret_box::seal(keyring, &hex::encode(raw), &user_mfa_context(auth.user_id))
        .map_err(|_| AppError::internal())?;
    match mfa::setup(
        &state.auth.db.pool,
        auth.user_id,
        auth.credential_id,
        &sealed,
        &hashes,
    )
    .await
    .map_err(internal)?
    {
        SetupOutcome::Stored => {}
        SetupOutcome::AlreadyEnabled => return Err(problem(ProblemCode::MfaAlreadyEnabled)),
        SetupOutcome::SessionGone => return Err(problem(ProblemCode::AuthenticationRequired)),
    }
    let secret = totp::base32_encode(&raw);
    Ok(Json(MfaSetupOutput {
        otpauth_uri: totp::otpauth_uri(&identity.totp_issuer, &auth.user.email, &secret),
        secret,
        recovery_codes: recovery
            .iter()
            .map(|c| totp::format_recovery_code(c))
            .collect(),
    }))
}

async fn enable(
    State(state): State<AppState>,
    Extension(identity): Extension<Arc<Identity>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<MfaEnableBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = session(&state, &headers, &jar).await?;
    limit(
        &state,
        format!("mfa-enable:{}", auth.user_id),
        MFA_ENABLE_PER_USER,
    )
    .await?;
    let Json(body) = body.map_err(AppError::from)?;
    let code = mfa_code(&body.code)?;
    let pool = &state.auth.db.pool;
    let row = mfa::find(pool, auth.user_id).await.map_err(internal)?;
    let Some(row) = row.filter(|r| r.enabled_at.is_none()) else {
        return Err(problem(ProblemCode::MfaNotSetup));
    };
    let secret = open_secret(&identity, &row)?;
    let Some(step) = totp::match_totp(&secret, &code, identity.now_ms()) else {
        return Err(problem(ProblemCode::MfaCodeInvalid));
    };
    let ip = peer_ip(peer.ip());
    match mfa::enable(
        pool,
        auth.user_id,
        auth.credential_id,
        &row.totp_secret,
        step,
        Some(&ip),
    )
    .await
    .map_err(internal)?
    {
        EnableOutcome::Ok => Ok(Json(OkResponse { ok: true })),
        EnableOutcome::NotSetup => Err(problem(ProblemCode::MfaNotSetup)),
        EnableOutcome::SessionGone => Err(problem(ProblemCode::AuthenticationRequired)),
    }
}

async fn disable(
    State(state): State<AppState>,
    Extension(identity): Extension<Arc<Identity>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<MfaDisableBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = session(&state, &headers, &jar).await?;
    limit(
        &state,
        format!("mfa-reauth:{}", auth.user_id),
        MFA_REAUTH_PER_USER,
    )
    .await?;
    let Json(body) = body.map_err(AppError::from)?;
    let current = optional_password(body.current_password)?;
    let code = body.code.as_deref().map(mfa_code).transpose()?;
    if current.is_none() && code.is_none() {
        return Err(AppError::with_source(ProblemCode::InvalidInput, "/"));
    }
    let pool = &state.auth.db.pool;
    let row = mfa::find(pool, auth.user_id).await.map_err(internal)?;
    let Some(row) = row.filter(|r| r.enabled_at.is_some()) else {
        return Err(problem(ProblemCode::MfaNotEnabled));
    };
    let confirmed = match confirm_password(&state, auth.user_id, current.as_deref()).await? {
        PasswordCheck::Ok => true,
        PasswordCheck::Invalid => false,
        // Password-less account: the current second factor re-authenticates.
        PasswordCheck::None => match code.as_deref() {
            Some(code) => match check_code(&identity, &row, code)? {
                Some((factor, hash)) => {
                    mfa::claim_factor_standalone(pool, auth.user_id, factor, hash.as_deref())
                        .await
                        .map_err(internal)?
                }
                None => false,
            },
            None => false,
        },
    };
    if !confirmed {
        return Err(problem(ProblemCode::MfaConfirmInvalid));
    }
    let ip = peer_ip(peer.ip());
    match mfa::disable(pool, auth.user_id, auth.credential_id, Some(&ip))
        .await
        .map_err(internal)?
    {
        DisableOutcome::Ok => Ok(Json(OkResponse { ok: true })),
        DisableOutcome::NotEnabled => Err(problem(ProblemCode::MfaNotEnabled)),
        DisableOutcome::SessionGone => Err(problem(ProblemCode::AuthenticationRequired)),
    }
}

async fn verify(
    State(state): State<AppState>,
    Extension(identity): Extension<Arc<Identity>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<MfaVerifyBody>, JsonRejection>,
) -> Result<Response, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let ip = peer_ip(peer.ip());
    limit(&state, format!("mfa-verify-ip:{ip}"), MFA_VERIFY_PER_IP).await?;
    let Json(body) = body.map_err(AppError::from)?;
    if body.mfa_token.is_empty() {
        return Err(AppError::with_source(
            ProblemCode::InvalidInput,
            "/mfaToken",
        ));
    }
    let code = mfa_code(&body.code)?;
    let pool = &state.auth.db.pool;
    let token_hash = hash_token(&body.mfa_token);
    let Some((user_id, generation)) = mfa::peek_challenge(pool, &token_hash)
        .await
        .map_err(internal)?
    else {
        return Err(problem(ProblemCode::MfaInvalid));
    };
    // The limit is per account, not per token: whoever knows the password can
    // mint new challenge tokens at will. It is counted in the database so it
    // holds across restarts and replicas (source: shared Redis limiter).
    if let Some(retry_after) =
        mfa::verify_attempt(pool, user_id, MFA_VERIFY_PER_USER, MFA_VERIFY_WINDOW_SECS)
            .await
            .map_err(internal)?
    {
        return Err(AppError::rate_limited(retry_after));
    }
    let row = mfa::find(pool, user_id).await.map_err(internal)?;
    let Some(row) = row.filter(|r| r.enabled_at.is_some()) else {
        return Err(problem(ProblemCode::MfaInvalid));
    };
    let Some((factor, recovery_hash)) = check_code(&identity, &row, &code)? else {
        return Err(problem(ProblemCode::MfaInvalid));
    };
    let issued = mfa::complete_challenge(
        pool,
        ChallengeCheck {
            token_hash: &token_hash,
            user_id,
            generation,
            secret: &row.totp_secret,
            factor,
            recovery_hash: recovery_hash.as_deref(),
        },
    )
    .await
    .map_err(internal)?;
    let Some((user_id, token)) = issued else {
        return Err(problem(ProblemCode::MfaInvalid));
    };
    let mut response = Json(SessionIssuedOutput {
        user_id: user_id.to_string(),
    })
    .into_response();
    if let Ok(value) = HeaderValue::from_str(&set_session_cookie(state.cookie_secure, &token)) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    Ok(response)
}
