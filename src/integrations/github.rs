//! GitHub App integration (source `core/github.ts`).
//!
//! Workspace admins install the app (a signed, single-use `state` bound to the
//! initiating session, round trip through github.com), link tasks to issues, and receive signed GitHub webhooks that
//! create/rename/close/reopen linked tasks. Status changes made in FVOCI are
//! pushed back to the linked issue by the `github` outbox consumer. All API
//! calls go to `GITHUB_API_URL` (default `https://api.github.com`), an
//! operator setting, so tests run against a local fake.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use chrono::Utc;
use hmac::{Hmac, Mac};
use regex::Regex;
use ring::hkdf;
use ring::rand::SystemRandom;
use ring::signature::{RsaKeyPair, RSA_PKCS1_SHA256};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::{PgPool, Postgres, Transaction};
use tracing::warn;
use url::Url;
use uuid::Uuid;

use crate::auth::password::Keyring;
use crate::db::backend::Backend;
use crate::db::context::{
    lock_membership_users, recheck_session, restore_system, set_system, set_tenant,
};
use crate::db::documents::{between, empty_document_json, DOCUMENT_SCHEMA_VERSION};
use crate::db::identity::{append_audit, append_event_channel, AuditAppend, EventAppend};
use crate::db::integrations::{require_manager_read, require_manager_write, IntegrationDbError};
use crate::db::outbox::{
    advance_cursor_backend_tx, advance_cursor_tx, BackendOutboxEvent, OutboxEvent,
};
use crate::db::projects::{lock_project, project_permission};
use crate::db::task_activity::record_task_activity;
use crate::db::workspace::workspace_is_live;
use crate::outbox::{DeliveryMode, OutboxConsumer, OutboxProcessError};
use crate::projects::ProjectPermission;
use crate::tasks::activity::ActivitySnapshot;

pub const GITHUB_CONSUMER: &str = "github";
pub const GITHUB_API_DEFAULT: &str = "https://api.github.com";
const GITHUB_WEB: &str = "https://github.com";
const STATE_TTL_MS: i64 = 600_000;
pub const STATE_SECRET_MIN_BYTES: usize = 32;
/// Serializes webhook and link writes for one `(workspace, repo, issue)`.
pub(crate) const ISSUE_LOCK_NAMESPACE: i32 = 1_907_030;
const TITLE_MAX: usize = 500;
/// Source 15 s; two calls per synced event must fit the 30 s outbox lease.
const GITHUB_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const GITHUB_RESPONSE_CAP: usize = 256 * 1024;
pub const GITHUB_WEBHOOK_BODY_MAX: usize = 1024 * 1024;

static REPO_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^/\s]+/[^/\s]+$").expect("repo regex"));
static PEM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^-----BEGIN (RSA PRIVATE KEY|PRIVATE KEY)-----\s+([A-Za-z0-9+/=\s]+?)\s+-----END (RSA PRIVATE KEY|PRIVATE KEY)-----$",
    )
    .expect("pem regex")
});

#[derive(Clone)]
pub struct GithubConfig {
    pub app_id: String,
    key: Arc<RsaKeyPair>,
    webhook_secret: String,
    /// Install `state` MAC key; server-only (see [`GithubConfig::from_env`]).
    state_key: [u8; 32],
    pub api_base: Url,
}

impl std::fmt::Debug for GithubConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubConfig")
            .field("app_id", &self.app_id)
            .field("private_key", &"<redacted>")
            .field("webhook_secret", &"<redacted>")
            .field("state_key", &"<redacted>")
            .field("api_base", &self.api_base.as_str())
            .finish()
    }
}

/// GitHub downloads PKCS#1 (`RSA PRIVATE KEY`); PKCS#8 is accepted too.
/// Literal `\n` sequences (single-line env values) become newlines.
pub fn parse_private_key(pem: &str) -> Result<RsaKeyPair, String> {
    let normalized = pem.replace("\\n", "\n");
    let caps = PEM_RE
        .captures(normalized.trim())
        .ok_or_else(|| "GITHUB_APP_PRIVATE_KEY is not a PEM RSA private key".to_string())?;
    if caps[1] != caps[3] {
        return Err("GITHUB_APP_PRIVATE_KEY PEM labels differ".into());
    }
    let der = STANDARD
        .decode(caps[2].split_whitespace().collect::<String>())
        .map_err(|_| "GITHUB_APP_PRIVATE_KEY is not valid base64".to_string())?;
    let parsed = if &caps[1] == "RSA PRIVATE KEY" {
        RsaKeyPair::from_der(&der)
    } else {
        RsaKeyPair::from_pkcs8(&der)
    };
    parsed.map_err(|err| format!("GITHUB_APP_PRIVATE_KEY rejected: {err}"))
}

/// Install-state MAC key derived from the active `ENCRYPTION_KEYS` key
/// (HKDF-SHA256, distinct salt/info, so it is never the sealing key itself).
pub fn derive_state_key(keys: &Keyring) -> [u8; 32] {
    let ikm = keys
        .keys
        .get(&keys.active_id)
        .expect("keyring active key present");
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, b"fvoci:github-install-state").extract(ikm);
    let mut out = [0u8; 32];
    prk.expand(&[b"v1"], hkdf::HKDF_SHA256)
        .and_then(|okm| okm.fill(&mut out))
        .expect("hkdf output length");
    out
}

/// `GITHUB_STATE_SECRET` (at least 32 bytes) keyed through HMAC to 32 bytes.
pub fn state_key_from_secret(secret: &str) -> Result<[u8; 32], String> {
    if secret.len() < STATE_SECRET_MIN_BYTES {
        return Err(format!(
            "GITHUB_STATE_SECRET must be at least {STATE_SECRET_MIN_BYTES} bytes"
        ));
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(b"fvoci:github-install-state:v1");
    Ok(mac.finalize().into_bytes().into())
}

/// Plain http is only for a loopback API (local fakes); anything else would
/// send the app JWT and installation tokens in clear.
fn api_base_is_allowed(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => match url.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
            None => false,
        },
        _ => false,
    }
}

impl GithubConfig {
    pub fn new(
        app_id: &str,
        private_key_pem: &str,
        webhook_secret: &str,
        api_base: &str,
        state_key: [u8; 32],
    ) -> Result<Self, String> {
        let app_id = app_id.trim();
        if app_id.is_empty() || webhook_secret.is_empty() {
            return Err("GITHUB_APP_ID and GITHUB_WEBHOOK_SECRET must be non-empty".into());
        }
        let api_base = Url::parse(api_base.trim().trim_end_matches('/'))
            .map_err(|err| format!("invalid GITHUB_API_URL: {err}"))?;
        if !api_base_is_allowed(&api_base) {
            return Err("GITHUB_API_URL must be https (http only for a loopback host)".into());
        }
        Ok(Self {
            app_id: app_id.to_string(),
            key: Arc::new(parse_private_key(private_key_pem)?),
            webhook_secret: webhook_secret.to_string(),
            state_key,
            api_base,
        })
    }

    /// `GITHUB_APP_ID`, `GITHUB_APP_PRIVATE_KEY`, `GITHUB_WEBHOOK_SECRET` all
    /// set, or none (source superRefine). The install-state key is
    /// `GITHUB_STATE_SECRET` when set, else derived from `ENCRYPTION_KEYS`
    /// (source used the server-only `SECRET_KEY`; the webhook secret is also
    /// known to GitHub App managers, so it is not used). Neither: boot fails.
    pub fn from_env(encryption_keys: Option<&Keyring>) -> Result<Option<Self>, String> {
        let read = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        };
        let fields = [
            read("GITHUB_APP_ID"),
            read("GITHUB_APP_PRIVATE_KEY"),
            read("GITHUB_WEBHOOK_SECRET"),
        ];
        match fields {
            [None, None, None] => Ok(None),
            [Some(id), Some(key), Some(secret)] => {
                let api =
                    read("GITHUB_API_URL").unwrap_or_else(|| GITHUB_API_DEFAULT.to_string());
                let state_key = match (read("GITHUB_STATE_SECRET"), encryption_keys) {
                    (Some(state_secret), _) => state_key_from_secret(&state_secret)?,
                    (None, Some(keys)) => derive_state_key(keys),
                    (None, None) => {
                        return Err(
                            "GitHub App needs GITHUB_STATE_SECRET or ENCRYPTION_KEYS for the install state key"
                                .into(),
                        )
                    }
                };
                Self::new(&id, &key, &secret, &api, state_key).map(Some)
            }
            _ => Err(
                "GITHUB_APP_ID, GITHUB_APP_PRIVATE_KEY, GITHUB_WEBHOOK_SECRET must all be set together, or none"
                    .into(),
            ),
        }
    }

    /// `GITHUB_API_URL` plus percent-encoded path segments.
    fn api(&self, segments: &[&str]) -> Url {
        let mut url = self.api_base.clone();
        url.path_segments_mut()
            .expect("http(s) base")
            .pop_if_empty()
            .extend(segments);
        url
    }

    /// Source `createGithubAppJwt`: RS256, `iat` now-60, `exp` now+540.
    pub fn app_jwt(&self) -> Result<String, String> {
        let now = Utc::now().timestamp();
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({ "iat": now - 60, "exp": now + 540, "iss": self.app_id }))
                .expect("jwt claims"),
        );
        let signing_input = format!("{header}.{claims}");
        let mut signature = vec![0u8; self.key.public().modulus_len()];
        self.key
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                signing_input.as_bytes(),
                &mut signature,
            )
            .map_err(|_| "github app jwt signing failed".to_string())?;
        Ok(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature)
        ))
    }

    /// Source `signInstallState` plus a nonce `n`: its hash is stored with the
    /// initiating user and session and consumed once by the callback.
    pub fn sign_install_state(&self, workspace_id: Uuid, nonce: &str, now_ms: i64) -> String {
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({
                "w": workspace_id.to_string(),
                "n": nonce,
                "e": now_ms + STATE_TTL_MS,
            }))
            .expect("state json"),
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.state_key).expect("hmac key");
        mac.update(payload.as_bytes());
        format!("{payload}.{}", hex::encode(mac.finalize().into_bytes()))
    }

    /// Returns the workspace and nonce of an authentic, unexpired state.
    pub fn verify_install_state(&self, state: &str, now_ms: i64) -> Option<(Uuid, String)> {
        let (payload, mac_hex) = state.rsplit_once('.')?;
        if payload.is_empty()
            || mac_hex.len() != 64
            || !mac_hex
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return None;
        }
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.state_key).expect("hmac key");
        mac.update(payload.as_bytes());
        mac.verify_slice(&hex::decode(mac_hex).ok()?).ok()?;
        let decoded: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
        let workspace_id = Uuid::parse_str(decoded.get("w")?.as_str()?).ok()?;
        let nonce = decoded.get("n")?.as_str().filter(|n| !n.is_empty())?;
        let expires = decoded.get("e")?.as_f64()?;
        if expires < now_ms as f64 {
            return None;
        }
        Some((workspace_id, nonce.to_string()))
    }

    /// Source `verifyGithubWebhookSignature`: `sha256=<64 lowercase hex>`,
    /// constant-time compare over the raw body.
    pub fn verify_webhook_signature(&self, body: &[u8], header: Option<&str>) -> bool {
        let Some(hex_mac) = header.and_then(|h| h.strip_prefix("sha256=")) else {
            return false;
        };
        if hex_mac.len() != 64
            || !hex_mac
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return false;
        }
        let Ok(expected) = hex::decode(hex_mac) else {
            return false;
        };
        let mut mac =
            Hmac::<Sha256>::new_from_slice(self.webhook_secret.as_bytes()).expect("hmac key");
        mac.update(body);
        mac.verify_slice(&expected).is_ok()
    }

    pub fn install_redirect(&self, slug: &str, state: &str) -> String {
        let mut url = Url::parse(GITHUB_WEB).expect("github url");
        url.path_segments_mut()
            .expect("base url")
            .extend(["apps", slug, "installations", "new"]);
        url.query_pairs_mut().append_pair("state", state);
        url.to_string()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GithubApiError {
    #[error("github request failed: {0}")]
    Transport(String),
    #[error("github responded {0}")]
    Status(u16),
    #[error("github response invalid")]
    Invalid,
}

async fn github_call(
    method: reqwest::Method,
    url: Url,
    bearer: &str,
    body: Option<Value>,
) -> Result<(u16, Option<Value>), GithubApiError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(GITHUB_REQUEST_TIMEOUT)
        .build()
        .map_err(|err| GithubApiError::Transport(err.without_url().to_string()))?;
    let mut request = client
        .request(method, url)
        .header("accept", "application/vnd.github+json")
        .header("authorization", format!("Bearer {bearer}"))
        .header("x-github-api-version", "2022-11-28")
        .header("user-agent", "FVOCI");
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|err| GithubApiError::Transport(err.without_url().to_string()))?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| GithubApiError::Transport(err.without_url().to_string()))?
    {
        if bytes.len() + chunk.len() > GITHUB_RESPONSE_CAP {
            return Ok((status, None));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((status, serde_json::from_slice(&bytes).ok()))
}

fn string_field(value: &Option<Value>, key: &str) -> Option<String> {
    value
        .as_ref()?
        .get(key)?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Source `fetchAppSlug`.
pub async fn fetch_app_slug(github: &GithubConfig) -> Result<String, GithubApiError> {
    let jwt = github.app_jwt().map_err(|_| GithubApiError::Invalid)?;
    let (status, body) =
        github_call(reqwest::Method::GET, github.api(&["app"]), &jwt, None).await?;
    if !(200..300).contains(&status) {
        return Err(GithubApiError::Status(status));
    }
    string_field(&body, "slug").ok_or(GithubApiError::Invalid)
}

/// `GET /app/installations/{id}` with the app JWT: whether the installation
/// exists and belongs to this app (404 otherwise).
pub async fn installation_exists(
    github: &GithubConfig,
    installation_id: &str,
) -> Result<bool, GithubApiError> {
    let jwt = github.app_jwt().map_err(|_| GithubApiError::Invalid)?;
    let (status, _) = github_call(
        reqwest::Method::GET,
        github.api(&["app", "installations", installation_id]),
        &jwt,
        None,
    )
    .await?;
    match status {
        200..=299 => Ok(true),
        404 => Ok(false),
        other => Err(GithubApiError::Status(other)),
    }
}

/// Source `installationToken`.
async fn installation_token(
    github: &GithubConfig,
    installation_id: &str,
) -> Result<String, GithubApiError> {
    let jwt = github.app_jwt().map_err(|_| GithubApiError::Invalid)?;
    let (status, body) = github_call(
        reqwest::Method::POST,
        github.api(&["app", "installations", installation_id, "access_tokens"]),
        &jwt,
        None,
    )
    .await?;
    if !(200..300).contains(&status) {
        return Err(GithubApiError::Status(status));
    }
    string_field(&body, "token").ok_or(GithubApiError::Invalid)
}

// ---------------------------------------------------------------------------
// Workspace routes (manage)

pub async fn get_installation(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Option<String>, IntegrationDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !require_manager_read(&mut tx, workspace_id, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    }
    let installation: Option<String> = sqlx::query_scalar(
        "SELECT installation_id FROM fvoci.github_installations WHERE workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(installation))
}

/// Manage check for `install` (the redirect URL is built outside the tx).
pub async fn check_manager(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let ok = require_manager_read(&mut tx, workspace_id, actor_user_id, session_id).await?;
    tx.commit().await?;
    Ok(ok)
}

/// A fresh install nonce: 256 random bits, base64url.
pub fn new_install_nonce() -> String {
    let mut bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Stores the install nonce hash for this admin session (valid as long as the
/// signed state). Expired rows of the workspace are dropped on the way.
pub async fn begin_install(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    nonce: &str,
) -> Result<Result<(), IntegrationDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !require_manager_write(&mut tx, workspace_id, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    }
    sqlx::query(
        "DELETE FROM fvoci.github_install_states WHERE workspace_id = $1 AND expires_at <= now()",
    )
    .bind(workspace_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO fvoci.github_install_states
            (nonce_hash, workspace_id, user_id, session_id, expires_at)
        VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5::double precision))
        "#,
    )
    .bind(crate::auth::token::hash_token(nonce))
    .bind(workspace_id)
    .bind(actor_user_id)
    .bind(session_id)
    .bind(STATE_TTL_MS as f64 / 1000.0)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn remove_installation(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), IntegrationDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !require_manager_write(&mut tx, workspace_id, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    }
    let removed: Option<String> = sqlx::query_scalar(
        "DELETE FROM fvoci.github_installations WHERE workspace_id = $1 RETURNING installation_id",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(installation_id) = removed else {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    };
    append_audit(
        &mut tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: "github.uninstalled".into(),
            target_type: None,
            target_id: None,
            payload: json!({ "installationId": installation_id }),
            ip: client_ip.map(str::to_string),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub fn installation_id_is_valid(raw: &str) -> bool {
    !raw.is_empty() && raw.len() <= 20 && raw.bytes().all(|b| b.is_ascii_digit())
}

/// Source `completeGithubInstall`, stricter: the nonce is consumed once and
/// only by the admin session that started the install; one installation per
/// workspace, one workspace per installation, and an existing link to another
/// installation is not replaced (uninstall first).
pub async fn complete_install(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    nonce: &str,
    installation_id: &str,
) -> Result<Result<(), IntegrationDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let consumed: Option<bool> = sqlx::query_scalar(
        r#"
        DELETE FROM fvoci.github_install_states
        WHERE nonce_hash = $1 AND workspace_id = $2 AND user_id = $3 AND session_id = $4
        RETURNING expires_at > now()
        "#,
    )
    .bind(crate::auth::token::hash_token(nonce))
    .bind(workspace_id)
    .bind(actor_user_id)
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await?;
    if consumed != Some(true) {
        // A consumed-but-expired nonce stays consumed.
        tx.commit().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    }
    if !require_manager_write(&mut tx, workspace_id, actor_user_id, session_id).await? {
        tx.commit().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    }
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT installation_id FROM fvoci.github_installations WHERE workspace_id = $1 FOR UPDATE",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    match existing.as_deref() {
        Some(current) if current == installation_id => {
            tx.commit().await?;
            return Ok(Ok(()));
        }
        Some(_) => {
            tx.commit().await?;
            return Ok(Err(IntegrationDbError::Conflict));
        }
        None => {}
    }
    let result = sqlx::query(
        r#"
        INSERT INTO fvoci.github_installations (id, workspace_id, installation_id)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(installation_id)
    .execute(&mut *tx)
    .await;
    match result {
        Ok(_) => {}
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            tx.rollback().await?;
            return Ok(Err(IntegrationDbError::Conflict));
        }
        Err(err) => return Err(err),
    }
    append_audit(
        &mut tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: "github.installed".into(),
            target_type: None,
            target_id: None,
            payload: json!({ "installationId": installation_id }),
            ip: None,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

#[derive(Debug, Clone)]
pub struct IssueLinkRow {
    pub id: Uuid,
    pub task_id: Uuid,
    pub repo: String,
    pub issue_number: i32,
}

pub fn repo_is_valid(repo: &str) -> bool {
    (3..=200).contains(&repo.chars().count()) && REPO_RE.is_match(repo)
}

/// Serializes the inbound create-or-update of one issue with a concurrent
/// delivery or manual link of the same issue, so the second one sees the first
/// one's link instead of failing on the unique index.
async fn lock_issue(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    repo: &str,
    issue_number: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
        .bind(ISSUE_LOCK_NAMESPACE)
        .bind(format!("{workspace_id}:{repo}#{issue_number}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Source `linkGithubIssue`: project edit permission on the task; a task and
/// an issue each link at most once (duplicates are 400).
pub async fn link_issue(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    task_id: Uuid,
    repo: &str,
    issue_number: i32,
) -> Result<Result<IssueLinkRow, IntegrationDbError>, sqlx::Error> {
    let repo = repo.trim();
    if !repo_is_valid(repo) || issue_number < 1 {
        return Ok(Err(IntegrationDbError::InvalidInput));
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    // Source checks project edit on the task (workspace.manage is only the
    // API-token scope of the route).
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await?
        || !workspace_is_live(&mut tx, workspace_id).await?
    {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    }
    let project_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(project_id) = project_id else {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    };
    let Some(project) = lock_project(&mut tx, workspace_id, project_id).await? else {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &project)
        .await?
        .at_least(ProjectPermission::Edit)
    {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    }
    lock_issue(&mut tx, workspace_id, repo, issue_number).await?;
    let id = Uuid::now_v7();
    let inserted = sqlx::query(
        r#"
        INSERT INTO fvoci.github_issue_links (id, workspace_id, task_id, repo, issue_number)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(task_id)
    .bind(repo)
    .bind(issue_number)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::InvalidInput));
    }
    tx.commit().await?;
    Ok(Ok(IssueLinkRow {
        id,
        task_id,
        repo: repo.to_string(),
        issue_number,
    }))
}

// ---------------------------------------------------------------------------
// Inbound GitHub webhook

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundOutcome {
    Ignored,
    Duplicate,
    Applied,
}

#[derive(Debug, thiserror::Error)]
pub enum InboundError {
    #[error("signature invalid")]
    Signature,
    #[error("payload invalid")]
    Invalid,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

struct Issue {
    repo: String,
    number: i32,
    title: String,
    state: String,
}

fn issue_from_payload(payload: &Value) -> Option<Issue> {
    let repo = payload.get("repository")?.get("full_name")?.as_str()?;
    let issue = payload
        .get("issue")
        .filter(|v| v.is_object())
        .or_else(|| payload.get("pull_request").filter(|v| v.is_object()))?;
    let number = issue.get("number")?.as_i64()?;
    if repo.is_empty() || !(1..=i64::from(i32::MAX)).contains(&number) {
        return None;
    }
    Some(Issue {
        repo: repo.to_string(),
        number: number as i32,
        title: issue
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        state: issue
            .get("state")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("open")
            .to_string(),
    })
}

fn clip_title(title: &str) -> String {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return "untitled".into();
    }
    trimmed.chars().take(TITLE_MAX).collect()
}

fn installation_id_of(payload: &Value) -> Option<String> {
    let id = payload.get("installation")?.get("id")?;
    match id {
        Value::Number(n) => n.as_u64().map(|n| n.to_string()),
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// Source `handleGithubWebhook`. The delivery mark and the task effects commit
/// together (source marked first in its own transaction, which dropped a
/// redelivery after a failed apply).
pub async fn handle_webhook(
    pool: &PgPool,
    github: &GithubConfig,
    body: &[u8],
    signature: Option<&str>,
    event_name: Option<&str>,
    delivery_id: Option<&str>,
) -> Result<InboundOutcome, InboundError> {
    if !github.verify_webhook_signature(body, signature) {
        return Err(InboundError::Signature);
    }
    if event_name == Some("ping") {
        return Ok(InboundOutcome::Ignored);
    }
    let payload: Value = serde_json::from_slice(body).map_err(|_| InboundError::Invalid)?;
    if !payload.is_object() {
        return Err(InboundError::Invalid);
    }
    let mut tx = pool.begin().await?;
    let previous = set_system(&mut tx).await?;
    if let Some(delivery) = delivery_id.and_then(|d| Uuid::parse_str(d).ok()) {
        let first = sqlx::query(
            "INSERT INTO fvoci.github_deliveries (delivery_id) VALUES ($1) ON CONFLICT DO NOTHING",
        )
        .bind(delivery)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if first == 0 {
            tx.rollback().await?;
            return Ok(InboundOutcome::Duplicate);
        }
    }
    let Some(installation_id) = installation_id_of(&payload) else {
        tx.commit().await?;
        return Ok(InboundOutcome::Ignored);
    };
    let workspace_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT workspace_id FROM fvoci.github_installations WHERE installation_id = $1",
    )
    .bind(&installation_id)
    .fetch_optional(&mut *tx)
    .await?;
    restore_system(&mut tx, &previous).await?;
    let Some(workspace_id) = workspace_id else {
        tx.commit().await?;
        return Ok(InboundOutcome::Ignored);
    };
    set_tenant(&mut tx, workspace_id).await?;
    let action = payload.get("action").and_then(Value::as_str);
    let outcome = match event_name {
        Some("installation") if matches!(action, Some("deleted" | "suspend")) => {
            sqlx::query("DELETE FROM fvoci.github_installations WHERE workspace_id = $1")
                .bind(workspace_id)
                .execute(&mut *tx)
                .await?;
            InboundOutcome::Applied
        }
        Some("issues" | "pull_request") => match action {
            Some(action) => apply_issue_event(&mut tx, workspace_id, action, &payload).await?,
            None => InboundOutcome::Ignored,
        },
        _ => InboundOutcome::Ignored,
    };
    tx.commit().await?;
    Ok(outcome)
}

async fn first_owner(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT m.user_id
        FROM fvoci.memberships AS m
        INNER JOIN fvoci.users AS u ON u.id = m.user_id AND u.deleted_at IS NULL
        WHERE m.workspace_id = $1
        ORDER BY (m.role = 'owner') DESC, m.created_at, m.user_id
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .fetch_optional(&mut **tx)
    .await
}

#[derive(Clone)]
struct StatusRow {
    id: Uuid,
    name: String,
    category: String,
}

async fn project_statuses(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Vec<StatusRow>, sqlx::Error> {
    let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
        r#"
        SELECT id, name, category FROM fvoci.statuses
        WHERE workspace_id = $1 AND project_id = $2
        ORDER BY sort_key COLLATE "C", id
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, category)| StatusRow { id, name, category })
        .collect())
}

fn default_status(statuses: &[StatusRow]) -> Option<&StatusRow> {
    statuses
        .iter()
        .find(|s| s.category == "backlog")
        .or_else(|| statuses.first())
}

fn done_status(statuses: &[StatusRow]) -> Option<&StatusRow> {
    statuses.iter().find(|s| s.category == "done")
}

async fn end_sort_key(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    status_id: Uuid,
    exclude: Option<Uuid>,
) -> Result<Option<String>, sqlx::Error> {
    let last: Option<String> = sqlx::query_scalar(
        r#"
        SELECT sort_key FROM fvoci.tasks
        WHERE workspace_id = $1 AND project_id = $2 AND status_id = $3
          AND deleted_at IS NULL AND ($4::uuid IS NULL OR id <> $4)
        ORDER BY sort_key COLLATE "C" DESC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(status_id)
    .bind(exclude)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(between(last.as_deref(), None).ok())
}

async fn record_webhook_task_event(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    verb: &str,
    task_id: Uuid,
    payload: Value,
) -> Result<(), sqlx::Error> {
    append_event_channel(
        tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: verb.to_string(),
            target_type: Some("task".into()),
            target_id: Some(task_id),
            payload: payload.clone(),
        },
        "webhook",
    )
    .await?;
    append_audit(
        tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: verb.to_string(),
            target_type: Some("task".into()),
            target_id: Some(task_id),
            payload,
            ip: None,
        },
    )
    .await
}

fn snapshot(title: &str, status: &StatusRow) -> ActivitySnapshot {
    let mut snap = ActivitySnapshot::new();
    snap.insert("title".into(), json!(title));
    snap.insert(
        "statusId".into(),
        json!({ "id": status.id.to_string(), "label": status.name }),
    );
    snap
}

/// Source `createLinkedTask`: first active project, backlog (or done when the
/// issue is closed).
async fn create_linked_task(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    issue: &Issue,
) -> Result<InboundOutcome, sqlx::Error> {
    let project_id: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT id FROM fvoci.projects
        WHERE workspace_id = $1 AND status = 'active' AND deleted_at IS NULL
        ORDER BY created_at, id
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(project_id) = project_id else {
        return Ok(InboundOutcome::Ignored);
    };
    let Some(project) = lock_project(tx, workspace_id, project_id).await? else {
        return Ok(InboundOutcome::Ignored);
    };
    if project.status != "active" {
        return Ok(InboundOutcome::Ignored);
    }
    let statuses = project_statuses(tx, workspace_id, project_id).await?;
    let status = if issue.state == "closed" {
        done_status(&statuses)
    } else {
        default_status(&statuses)
    };
    let Some(status) = status.cloned() else {
        return Ok(InboundOutcome::Ignored);
    };
    let Some(sort_key) = end_sort_key(tx, workspace_id, project_id, status.id, None).await? else {
        return Ok(InboundOutcome::Ignored);
    };
    let number: i32 = sqlx::query_scalar(
        r#"
        UPDATE fvoci.projects SET next_number = next_number + 1, updated_at = now()
        WHERE workspace_id = $1 AND id = $2
        RETURNING next_number - 1
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    let title = clip_title(&issue.title);
    let task_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.tasks (
            id, workspace_id, project_id, number, title, type, priority, status_id,
            sort_key, schema_version, content_json, created_by
        ) VALUES ($1, $2, $3, $4, $5, 'task', 'none', $6, $7, $8, $9, $10)
        "#,
    )
    .bind(task_id)
    .bind(workspace_id)
    .bind(project_id)
    .bind(number)
    .bind(&title)
    .bind(status.id)
    .bind(&sort_key)
    .bind(DOCUMENT_SCHEMA_VERSION)
    .bind(empty_document_json())
    .bind(actor_user_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO fvoci.github_issue_links (id, workspace_id, task_id, repo, issue_number)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(task_id)
    .bind(&issue.repo)
    .bind(issue.number)
    .execute(&mut **tx)
    .await?;
    record_webhook_task_event(
        tx,
        workspace_id,
        actor_user_id,
        "task.created",
        task_id,
        json!({
            "taskId": task_id.to_string(),
            "projectId": project_id.to_string(),
            "number": number,
            "statusId": status.id.to_string(),
            "title": title,
        }),
    )
    .await?;
    record_task_activity(
        tx,
        workspace_id,
        task_id,
        actor_user_id,
        "webhook",
        None,
        &ActivitySnapshot::new(),
    )
    .await?;
    Ok(InboundOutcome::Applied)
}

/// Source `handleIssueEvent` (+ `applyIssueState`).
async fn apply_issue_event(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    action: &str,
    payload: &Value,
) -> Result<InboundOutcome, sqlx::Error> {
    let Some(issue) = issue_from_payload(payload) else {
        return Ok(InboundOutcome::Ignored);
    };
    if !matches!(action, "opened" | "edited" | "closed" | "reopened") {
        return Ok(InboundOutcome::Ignored);
    }
    let Some(actor_user_id) = first_owner(tx, workspace_id).await? else {
        return Ok(InboundOutcome::Ignored);
    };
    lock_issue(tx, workspace_id, &issue.repo, issue.number).await?;
    let linked: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT task_id FROM fvoci.github_issue_links
        WHERE workspace_id = $1 AND repo = $2 AND issue_number = $3
        "#,
    )
    .bind(workspace_id)
    .bind(&issue.repo)
    .bind(issue.number)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(task_id) = linked else {
        if matches!(action, "opened" | "edited") {
            return create_linked_task(tx, workspace_id, actor_user_id, &issue).await;
        }
        return Ok(InboundOutcome::Ignored);
    };
    let initial: Option<Uuid> = sqlx::query_scalar(
        "SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(project_id) = initial else {
        return Ok(InboundOutcome::Ignored);
    };
    let Some(project) = lock_project(tx, workspace_id, project_id).await? else {
        return Ok(InboundOutcome::Ignored);
    };
    let task: Option<(Uuid, String, Uuid, bool)> = sqlx::query_as(
        r#"
        SELECT project_id, title, status_id, archived_at IS NOT NULL
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        FOR NO KEY UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((task_project, title, status_id, archived)) = task else {
        return Ok(InboundOutcome::Ignored);
    };
    // Source assertTaskWritable: archived task or project is left alone.
    if task_project != project_id || archived || project.status == "archived" {
        return Ok(InboundOutcome::Ignored);
    }
    let statuses = project_statuses(tx, workspace_id, project_id).await?;
    let Some(current) = statuses.iter().find(|s| s.id == status_id).cloned() else {
        return Ok(InboundOutcome::Ignored);
    };
    let before = snapshot(&title, &current);
    let mut new_title = title.clone();
    let mut changed = false;
    if matches!(action, "opened" | "edited") && !issue.title.is_empty() {
        let clipped = clip_title(&issue.title);
        if clipped != title {
            sqlx::query(
                r#"
                UPDATE fvoci.tasks SET title = $3, version = version + 1, updated_at = now()
                WHERE workspace_id = $1 AND id = $2
                "#,
            )
            .bind(workspace_id)
            .bind(task_id)
            .bind(&clipped)
            .execute(&mut **tx)
            .await?;
            record_webhook_task_event(
                tx,
                workspace_id,
                actor_user_id,
                "task.updated",
                task_id,
                json!({ "taskId": task_id.to_string(), "title": clipped }),
            )
            .await?;
            new_title = clipped;
            changed = true;
        }
    }
    let mut new_status = current.clone();
    if action != "edited" {
        let want_closed = action == "closed" || (action == "opened" && issue.state == "closed");
        let target = if want_closed {
            done_status(&statuses)
        } else {
            default_status(&statuses)
        };
        let allowed = if want_closed {
            current.category != "done"
        } else {
            matches!(current.category.as_str(), "done" | "canceled")
        };
        if let Some(target) = target.filter(|t| allowed && t.id != current.id).cloned() {
            if let Some(sort_key) =
                end_sort_key(tx, workspace_id, project_id, target.id, Some(task_id)).await?
            {
                sqlx::query(
                    r#"
                    UPDATE fvoci.tasks
                    SET status_id = $3, sort_key = $4, version = version + 1, updated_at = now()
                    WHERE workspace_id = $1 AND id = $2
                    "#,
                )
                .bind(workspace_id)
                .bind(task_id)
                .bind(target.id)
                .bind(&sort_key)
                .execute(&mut **tx)
                .await?;
                record_webhook_task_event(
                    tx,
                    workspace_id,
                    actor_user_id,
                    "task.updated",
                    task_id,
                    json!({
                        "taskId": task_id.to_string(),
                        "from": current.id.to_string(),
                        "to": target.id.to_string(),
                    }),
                )
                .await?;
                new_status = target;
                changed = true;
            }
        }
    }
    if !changed {
        return Ok(InboundOutcome::Ignored);
    }
    record_task_activity(
        tx,
        workspace_id,
        task_id,
        actor_user_id,
        "webhook",
        Some(&before),
        &snapshot(&new_title, &new_status),
    )
    .await?;
    Ok(InboundOutcome::Applied)
}

// ---------------------------------------------------------------------------
// Outbound status sync (source `syncLinkedGithubIssue`)

struct SyncTarget {
    installation_id: String,
    repo: String,
    issue_number: i32,
    state: &'static str,
}

async fn sync_target(
    pool: &PgPool,
    event: &OutboxEvent,
) -> Result<Option<SyncTarget>, sqlx::Error> {
    // Inbound webhook effects are not echoed back (source channel check).
    if event.channel == "webhook" || event.verb != "task.updated" {
        return Ok(None);
    }
    let (Some(workspace_id), Some(task_id)) = (event.workspace_id, event.target_id) else {
        return Ok(None);
    };
    if !event.payload.get("to").is_some_and(Value::is_string) {
        return Ok(None);
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let row: Option<(String, i32, String, String)> = sqlx::query_as(
        r#"
        SELECT l.repo, l.issue_number, s.category, i.installation_id
        FROM fvoci.github_issue_links AS l
        INNER JOIN fvoci.tasks AS t ON t.workspace_id = l.workspace_id AND t.id = l.task_id
        INNER JOIN fvoci.statuses AS s ON s.workspace_id = t.workspace_id AND s.id = t.status_id
        INNER JOIN fvoci.github_installations AS i ON i.workspace_id = l.workspace_id
        WHERE l.workspace_id = $1 AND l.task_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row.map(
        |(repo, issue_number, category, installation_id)| SyncTarget {
            installation_id,
            repo,
            issue_number,
            state: if matches!(category.as_str(), "done" | "canceled") {
                "closed"
            } else {
                "open"
            },
        },
    ))
}

/// `owner/name` as two path segments; `.`/`..` would change the request path.
fn repo_segments(repo: &str) -> Option<(&str, &str)> {
    let (owner, name) = repo.split_once('/')?;
    let dotted = |segment: &str| segment == "." || segment == "..";
    (!owner.is_empty() && !name.is_empty() && !dotted(owner) && !dotted(name))
        .then_some((owner, name))
}

/// Source `syncLinkedGithubIssue`. Client errors (token or PATCH) are final:
/// the event is one per status change on the shared `github` cursor, so a
/// workspace whose app was removed or suspended on GitHub must not hold other
/// workspaces behind its retries. 5xx and transport errors are retried.
pub async fn sync_linked_issue(
    pool: &PgPool,
    github: &GithubConfig,
    event: &OutboxEvent,
) -> Result<(), OutboxProcessError> {
    let Some(target) = sync_target(pool, event).await? else {
        return Ok(());
    };
    sync_current_target(github, event.id, target).await
}

// Shared maintained token/PATCH flow for legacy PG and selected Backend entrypoints.
async fn sync_current_target(
    github: &GithubConfig,
    event_id: Uuid,
    target: SyncTarget,
) -> Result<(), OutboxProcessError> {
    let Some((owner, name)) = repo_segments(&target.repo) else {
        warn!(event_id = %event_id, "github.repo_unusable");
        return Ok(());
    };
    let token = match installation_token(github, &target.installation_id).await {
        Ok(token) => token,
        Err(GithubApiError::Status(status)) if (400..500).contains(&status) => {
            warn!(event_id = %event_id, http_status = status, "github.token_refused");
            return Ok(());
        }
        Err(err) => {
            return Err(OutboxProcessError::Delivery(format!("github token: {err}")));
        }
    };
    let issue_number = target.issue_number.to_string();
    let (status, _) = github_call(
        reqwest::Method::PATCH,
        github.api(&["repos", owner, name, "issues", &issue_number]),
        &token,
        Some(json!({ "state": target.state })),
    )
    .await
    .map_err(|err| OutboxProcessError::Delivery(format!("github patch: {err}")))?;
    if (400..500).contains(&status) {
        // Source: a client error is final (not retried).
        warn!(event_id = %event_id, http_status = status, "github.request_failed");
        return Ok(());
    }
    if !(200..300).contains(&status) {
        return Err(OutboxProcessError::Delivery(format!(
            "github patch status {status}"
        )));
    }
    Ok(())
}

fn sync_target_ids(event: &BackendOutboxEvent) -> Option<(Uuid, Uuid)> {
    if event.channel == "webhook"
        || event.verb != "task.updated"
        || !event.payload.get("to").is_some_and(Value::is_string)
    {
        return None;
    }
    Some((event.workspace_id?, event.target_id?))
}

async fn sync_target_backend(
    backend: &Backend,
    event: &BackendOutboxEvent,
) -> Result<Option<SyncTarget>, sqlx::Error> {
    let Some((workspace, task)) = sync_target_ids(event) else {
        return Ok(None);
    };
    let mut tx = backend.begin_read().await?;
    tx.operation().set_tenant(workspace).await?;
    let row = tx.operation().github_sync_target(workspace, task).await?;
    tx.commit()
        .await
        .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    Ok(row.map(
        |(repo, issue_number, category, installation_id)| SyncTarget {
            installation_id,
            repo,
            issue_number,
            state: if matches!(category.as_str(), "done" | "canceled") {
                "closed"
            } else {
                "open"
            },
        },
    ))
}

async fn sync_linked_issue_backend(
    backend: &Backend,
    github: &GithubConfig,
    event: &BackendOutboxEvent,
) -> Result<(), OutboxProcessError> {
    let Some(target) = sync_target_backend(backend, event).await? else {
        return Ok(());
    };
    // The current target read is confirmed and its connection released before either request.
    sync_current_target(github, event.id, target).await
}

async fn skip_event_backend(
    backend: &Backend,
    lease_owner: Uuid,
    event: &BackendOutboxEvent,
) -> Result<(), OutboxProcessError> {
    let mut tx = backend.begin_write().await?;
    let previous = tx.operation().set_system().await?;
    if !advance_cursor_backend_tx(&mut tx, GITHUB_CONSUMER, lease_owner, &event.cursor()).await? {
        tx.rollback().await?;
        return Err(OutboxProcessError::Delivery(
            "advance rejected in backend tx".into(),
        ));
    }
    tx.operation().restore_system(previous).await?;
    tx.commit()
        .await
        .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    Ok(())
}

/// With the app configured, pushes status changes to GitHub. Without it, the
/// same cursor only moves forward (DatabaseAtomic, no effect), so configuring the app
/// later does not replay every status change recorded while it was off.
pub struct GithubSyncConsumer {
    github: Option<GithubConfig>,
}

pub fn github_sync_consumer(github: Option<GithubConfig>) -> Arc<dyn OutboxConsumer> {
    Arc::new(GithubSyncConsumer { github })
}

async fn skip_event(
    pool: &PgPool,
    lease_owner: Uuid,
    event: &OutboxEvent,
) -> Result<(), OutboxProcessError> {
    let mut tx = pool.begin().await?;
    if !advance_cursor_tx(
        &mut tx,
        GITHUB_CONSUMER,
        lease_owner,
        &event.xact,
        event.seq,
    )
    .await?
    {
        tx.rollback().await?;
        return Err(OutboxProcessError::Delivery(
            "advance rejected in pg-only tx".into(),
        ));
    }
    tx.commit().await?;
    Ok(())
}

impl OutboxConsumer for GithubSyncConsumer {
    fn name(&self) -> &str {
        GITHUB_CONSUMER
    }

    fn delivery_mode(&self) -> DeliveryMode {
        if self.github.is_some() {
            DeliveryMode::External
        } else {
            DeliveryMode::DatabaseAtomic
        }
    }

    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            match &self.github {
                Some(github) => sync_linked_issue(pool, github, event).await,
                None => skip_event(pool, lease_owner, event).await,
            }
        })
    }

    fn deliver_backend<'a>(
        &'a self,
        backend: &'a Backend,
        lease_owner: Uuid,
        event: &'a BackendOutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            match &self.github {
                Some(github) => sync_linked_issue_backend(backend, github, event).await,
                None => skip_event_backend(backend, lease_owner, event).await,
            }
        })
    }

    fn deliver_batch_backend<'a>(
        &'a self,
        backend: &'a Backend,
        lease_owner: Uuid,
        events: &'a [BackendOutboxEvent],
    ) -> Pin<Box<dyn Future<Output = (usize, Option<OutboxProcessError>)> + Send + 'a>> {
        Box::pin(async move {
            // Select the same implementation for PG and family; the trait's
            // compatibility default would route PG back to its old target read.
            let mut done = 0;
            for event in events {
                if let Err(error) = self.deliver_backend(backend, lease_owner, event).await {
                    return (done, Some(error));
                }
                done += 1;
            }
            (done, None)
        })
    }

    /// One event per call: its two requests (token + PATCH, 10 s each) fit
    /// the dispatcher's lease budget.
    fn batch_event_cap(&self) -> usize {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY: &str = include_str!("../../tests/fixtures/github-app-test-key.pem");

    fn config() -> GithubConfig {
        GithubConfig::new("123", TEST_KEY, "whsec", "http://127.0.0.1:9", [7u8; 32])
            .expect("config")
    }

    #[test]
    fn state_round_trip_expiry_and_tamper() {
        let github = config();
        let ws = Uuid::now_v7();
        let state = github.sign_install_state(ws, "nonce-1", 1_000);
        assert_eq!(
            github.verify_install_state(&state, 1_000),
            Some((ws, "nonce-1".to_string()))
        );
        assert_eq!(
            github.verify_install_state(&state, 1_000 + STATE_TTL_MS + 1),
            None
        );
        let mut tampered = state.clone();
        tampered.insert(0, 'x');
        assert_eq!(github.verify_install_state(&tampered, 1_000), None);
        // Same webhook secret, other state key: the webhook secret alone
        // cannot mint install state.
        let other = GithubConfig::new("123", TEST_KEY, "whsec", "http://127.0.0.1:9", [8u8; 32])
            .expect("c");
        assert_eq!(other.verify_install_state(&state, 1_000), None);
    }

    #[test]
    fn state_key_sources() {
        assert!(state_key_from_secret(&"s".repeat(STATE_SECRET_MIN_BYTES - 1)).is_err());
        let a = state_key_from_secret(&"s".repeat(STATE_SECRET_MIN_BYTES)).expect("key");
        let b = state_key_from_secret(&"t".repeat(STATE_SECRET_MIN_BYTES)).expect("key");
        assert_ne!(a, b);
        let keys = Keyring::parse_named(
            r#"{"k1":"0101010101010101010101010101010101010101010101010101010101010101","k2":"0202020202020202020202020202020202020202020202020202020202020202"}"#,
            "k1",
            "ENCRYPTION_KEYS",
        )
        .expect("keys");
        let derived = derive_state_key(&keys);
        assert_ne!(derived.as_slice(), keys.keys["k1"].as_slice());
        assert_eq!(derived, derive_state_key(&keys));
    }

    #[test]
    fn api_base_is_https_or_loopback_http() {
        let make = |api: &str| GithubConfig::new("1", TEST_KEY, "s", api, [1u8; 32]);
        assert!(make("https://api.github.com").is_ok());
        assert!(make("https://ghe.example.com/api/v3").is_ok());
        assert!(make("http://127.0.0.1:9").is_ok());
        assert!(make("http://[::1]:9").is_ok());
        assert!(make("http://localhost:9").is_ok());
        assert!(make("http://api.github.com").is_err());
        assert!(make("http://10.0.0.1").is_err());
        assert!(make("ftp://api.github.com").is_err());
    }

    #[test]
    fn api_paths_are_percent_encoded_segments() {
        let github = GithubConfig::new(
            "1",
            TEST_KEY,
            "s",
            "https://ghe.example.com/api/v3/",
            [1u8; 32],
        )
        .expect("config");
        assert_eq!(
            github.api(&["repos", "o", "a?b#c", "issues", "7"]).as_str(),
            "https://ghe.example.com/api/v3/repos/o/a%3Fb%23c/issues/7"
        );
        assert_eq!(repo_segments("octo/repo"), Some(("octo", "repo")));
        assert_eq!(repo_segments("../x"), None);
        assert_eq!(repo_segments("o/.."), None);
    }

    #[test]
    fn webhook_signature_requires_exact_lowercase_hex() {
        let github = config();
        let body = br#"{"zen":"ok"}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"whsec").expect("key");
        mac.update(body);
        let good = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        assert!(github.verify_webhook_signature(body, Some(&good)));
        assert!(!github.verify_webhook_signature(b"{}", Some(&good)));
        assert!(!github.verify_webhook_signature(body, Some(&good.to_uppercase())));
        assert!(!github.verify_webhook_signature(body, None));
        assert!(!github.verify_webhook_signature(body, Some("sha1=abc")));
    }

    #[test]
    fn jwt_verifies_with_the_app_public_key() {
        let github = config();
        let jwt = github.app_jwt().expect("jwt");
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).expect("b64")).expect("json");
        assert_eq!(claims["iss"], "123");
        let signature = URL_SAFE_NO_PAD.decode(parts[2]).expect("sig");
        let public = ring::signature::UnparsedPublicKey::new(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            github.key.public().as_ref().to_vec(),
        );
        public
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .expect("signature");
    }

    #[test]
    fn repo_and_installation_rules() {
        assert!(repo_is_valid("octo/repo"));
        assert!(!repo_is_valid("octo"));
        assert!(!repo_is_valid("octo/re po"));
        assert!(!repo_is_valid("a/b/c"));
        assert!(installation_id_is_valid("12345"));
        assert!(!installation_id_is_valid("12a"));
        assert!(!installation_id_is_valid(""));
        assert!(GithubConfig::new("1", "not a key", "s", GITHUB_API_DEFAULT, [1u8; 32]).is_err());
    }
}

#[cfg(test)]
mod backend_regressions {
    use super::*;
    use crate::db::notifications::family_runtime_fixture::Fixture;
    use crate::db::outbox::{
        ensure_consumer_backend, fetch_cursor_backend, fetch_event_by_id_backend,
        is_processed_backend, lease_consumer_backend, release_consumer_backend, OutboxCursor,
    };
    use axum::{
        body::Bytes,
        extract::{OriginalUri, State},
        http::{HeaderMap, Method, StatusCode},
        routing::any,
        Json, Router,
    };
    use std::sync::{
        atomic::{AtomicBool, AtomicU16, Ordering},
        Mutex,
    };
    use tokio::sync::Notify;

    const TEST_KEY: &str = include_str!("../../tests/fixtures/github-app-test-key.pem");
    const TOKEN: &str = "synthetic-installation-token";
    #[derive(Clone, Debug)]
    struct Call {
        method: String,
        path: String,
        body: Value,
        authenticated: bool,
        headers_valid: bool,
    }
    #[derive(Clone)]
    struct ProviderState {
        calls: Arc<Mutex<Vec<Call>>>,
        token_status: Arc<AtomicU16>,
        patch_status: Arc<AtomicU16>,
        hold_patch: Arc<AtomicBool>,
        patch_seen: Arc<Notify>,
        release_patch: Arc<Notify>,
        github: GithubConfig,
    }
    fn app_bearer_valid(headers: &HeaderMap, github: &GithubConfig) -> bool {
        let Some(jwt) = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
        else {
            return false;
        };
        let parts: Vec<_> = jwt.split('.').collect();
        if parts.len() != 3 {
            return false;
        }
        let Ok(signature) = URL_SAFE_NO_PAD.decode(parts[2]) else {
            return false;
        };
        let Ok(claims) = URL_SAFE_NO_PAD.decode(parts[1]) else {
            return false;
        };
        let Ok(claims) = serde_json::from_slice::<Value>(&claims) else {
            return false;
        };
        claims["iss"] == "123"
            && ring::signature::UnparsedPublicKey::new(
                &ring::signature::RSA_PKCS1_2048_8192_SHA256,
                github.key.public().as_ref(),
            )
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .is_ok()
    }
    async fn receive(
        State(state): State<ProviderState>,
        method: Method,
        OriginalUri(uri): OriginalUri,
        headers: HeaderMap,
        bytes: Bytes,
    ) -> (StatusCode, Json<Value>) {
        let token_request = method == Method::POST;
        let authenticated = if token_request {
            app_bearer_valid(&headers, &state.github)
        } else {
            headers
                .get("authorization")
                .is_some_and(|v| v == format!("Bearer {TOKEN}").as_str())
        };
        let headers_valid = headers
            .get("accept")
            .is_some_and(|v| v == "application/vnd.github+json")
            && headers
                .get("x-github-api-version")
                .is_some_and(|v| v == "2022-11-28")
            && headers.get("user-agent").is_some_and(|v| v == "FVOCI");
        state.calls.lock().unwrap().push(Call {
            method: method.to_string(),
            path: uri.path().into(),
            body: if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap()
            },
            authenticated,
            headers_valid,
        });
        if method == Method::PATCH {
            state.patch_seen.notify_one();
            if state.hold_patch.swap(false, Ordering::SeqCst) {
                state.release_patch.notified().await;
            }
        }
        let status = if token_request {
            state.token_status.load(Ordering::SeqCst)
        } else {
            state.patch_status.load(Ordering::SeqCst)
        };
        (
            StatusCode::from_u16(status).unwrap(),
            Json(if token_request {
                json!({"token": TOKEN})
            } else {
                json!({"state": "ok"})
            }),
        )
    }
    struct Provider {
        state: ProviderState,
        join: tokio::task::JoinHandle<()>,
    }
    impl Provider {
        async fn new() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let github = GithubConfig::new(
                "123",
                TEST_KEY,
                "synthetic-webhook-secret",
                &format!("http://{address}/api/v3"),
                [7; 32],
            )
            .unwrap();
            let state = ProviderState {
                calls: Arc::new(Mutex::new(Vec::new())),
                token_status: Arc::new(AtomicU16::new(201)),
                patch_status: Arc::new(AtomicU16::new(200)),
                hold_patch: Arc::new(AtomicBool::new(false)),
                patch_seen: Arc::new(Notify::new()),
                release_patch: Arc::new(Notify::new()),
                github,
            };
            let app = Router::new()
                .fallback(any(receive))
                .with_state(state.clone());
            let join = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            Self { state, join }
        }
        fn config(&self) -> GithubConfig {
            self.state.github.clone()
        }
        fn calls(&self) -> Vec<Call> {
            self.state.calls.lock().unwrap().clone()
        }
        async fn finish(self) {
            self.state.release_patch.notify_one();
            self.join.abort();
            assert!(self.join.await.unwrap_err().is_cancelled());
        }
    }
    struct TaskFixture {
        f: Fixture,
        task: Uuid,
        statuses: Vec<Uuid>,
    }
    impl TaskFixture {
        async fn new(repo: &str) -> Self {
            let f = Fixture::new().await;
            let version: String = sqlx::query_scalar("SELECT sqlite_version()")
                .fetch_one(&f.pool)
                .await
                .unwrap();
            assert_eq!(version, crate::db::pool::SQLITE_VERSION);
            let source: String = sqlx::query_scalar("SELECT sqlite_source_id()")
                .fetch_one(&f.pool)
                .await
                .unwrap();
            assert_eq!(source, crate::db::pool::SQLITE_SOURCE_ID);
            let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
                .fetch_one(&f.pool)
                .await
                .unwrap();
            assert_eq!(fk, 1);
            let project = Uuid::now_v7();
            let workflow = Uuid::now_v7();
            let task = Uuid::now_v7();
            sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'GITHUB','synthetic github','workspace',?3)")
                .bind(project.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.actor.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
            sqlx::query("INSERT INTO workflows(id,workspace_id,project_id) VALUES(?1,?2,?3)")
                .bind(workflow.as_bytes().as_slice())
                .bind(f.workspace.as_bytes().as_slice())
                .bind(project.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            let mut statuses = Vec::new();
            for (n, category) in ["backlog", "todo", "in_progress", "done", "canceled"]
                .iter()
                .enumerate()
            {
                let id = Uuid::now_v7();
                sqlx::query("INSERT INTO statuses(id,workspace_id,project_id,workflow_id,name,category,sort_key) VALUES(?1,?2,?3,?4,?5,?5,?6)")
                    .bind(id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(workflow.as_bytes().as_slice()).bind(category).bind(n.to_string()).execute(&f.pool).await.unwrap();
                statuses.push(id);
            }
            sqlx::query("INSERT INTO tasks(id,workspace_id,project_id,number,title,status_id,content_json,created_by) VALUES(?1,?2,?3,1,'synthetic 한글🙂',?4,'{\"type\":\"doc\"}',?5)")
                .bind(task.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(statuses[3].as_bytes().as_slice()).bind(f.actor.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
            sqlx::query("INSERT INTO github_installations(id,workspace_id,installation_id) VALUES(?1,?2,'42')")
                .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
            sqlx::query("INSERT INTO github_issue_links(id,workspace_id,task_id,repo,issue_number) VALUES(?1,?2,?3,?4,7)")
                .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(task.as_bytes().as_slice()).bind(repo).execute(&f.pool).await.unwrap();
            Self { f, task, statuses }
        }
        async fn event(
            &self,
            verb: &str,
            workspace: Option<Uuid>,
            target: Option<Uuid>,
            payload: Value,
            channel: &str,
        ) -> BackendOutboxEvent {
            let id = Uuid::now_v7();
            let mut tx = self.f.backend.begin_write().await.unwrap();
            if let Some(workspace) = workspace {
                tx.operation().set_tenant(workspace).await.unwrap()
            }
            let previous = tx.operation().set_system().await.unwrap();
            tx.operation()
                .append_event(EventAppend {
                    id,
                    workspace_id: workspace,
                    actor_user_id: Some(self.f.actor),
                    verb: verb.into(),
                    target_type: Some("task".into()),
                    target_id: target,
                    payload,
                })
                .await
                .unwrap();
            tx.operation().restore_system(previous).await.unwrap();
            tx.commit().await.unwrap();
            if channel != "web" {
                sqlx::query("UPDATE events SET channel=?1 WHERE id=?2")
                    .bind(channel)
                    .bind(id.as_bytes().as_slice())
                    .execute(&self.f.pool)
                    .await
                    .unwrap();
            }
            fetch_event_by_id_backend(&self.f.backend, id)
                .await
                .unwrap()
                .unwrap()
        }
        async fn change(&self) -> BackendOutboxEvent {
            self.event(
                "task.updated",
                Some(self.f.workspace),
                Some(self.task),
                json!({"to":self.statuses[0]}),
                "web",
            )
            .await
        }
        async fn cursor(&self) -> Option<OutboxCursor> {
            fetch_cursor_backend(&self.f.backend, GITHUB_CONSUMER)
                .await
                .unwrap()
        }
        async fn protected(&self) -> (i64, i64, i64, i64) {
            sqlx::query_as("SELECT (SELECT count(*) FROM github_issue_links),(SELECT count(*) FROM github_installations),(SELECT count(*) FROM processed_events WHERE consumer='github'),(SELECT count(*) FROM outbox_failures WHERE consumer='github')")
                .fetch_one(&self.f.pool).await.unwrap()
        }
        async fn finish(self) {
            self.f.finish().await;
        }
    }
    async fn lease(f: &TaskFixture, backend: &Backend) -> Uuid {
        ensure_consumer_backend(backend, GITHUB_CONSUMER)
            .await
            .unwrap();
        let owner = Uuid::now_v7();
        assert!(lease_consumer_backend(backend, GITHUB_CONSUMER, owner, 30)
            .await
            .unwrap());
        assert_eq!(
            f.cursor().await,
            Some(OutboxCursor::SqliteFamily { seq: 0 })
        );
        owner
    }
    fn dispatcher(
        f: &TaskFixture,
        config: Option<GithubConfig>,
    ) -> crate::outbox::OutboxDispatcherHandle {
        crate::outbox::spawn_outbox_dispatcher_backend(
            crate::outbox::OutboxDispatcherSettings {
                poll_interval: Duration::from_millis(10),
                ..Default::default()
            },
            f.f.backend.clone(),
            vec![github_sync_consumer(config)],
        )
        .unwrap()
    }
    async fn wait_cursor(f: &TaskFixture, event: &BackendOutboxEvent) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if f.cursor().await == Some(event.cursor()) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn wait_mark(f: &TaskFixture, event: &BackendOutboxEvent) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if is_processed_backend(&f.f.backend, GITHUB_CONSUMER, event.id)
                    .await
                    .unwrap()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    fn assert_calls(calls: &[Call], state: &str, repo_path: &str) {
        assert_eq!(calls.len(), 2);
        assert_eq!(
            (calls[0].method.as_str(), calls[0].path.as_str()),
            ("POST", "/api/v3/app/installations/42/access_tokens")
        );
        assert_eq!(calls[0].body, Value::Null);
        assert_eq!(
            (calls[1].method.as_str(), calls[1].path.as_str()),
            ("PATCH", repo_path)
        );
        assert_eq!(calls[1].body, json!({"state":state}));
        assert!(calls.iter().all(|c| c.authenticated && c.headers_valid));
    }

    #[tokio::test]
    async fn actual_backend_unconfigured_registered_cursor_commits_without_mark_or_replay() {
        let f = TaskFixture::new("octo/repo").await;
        let provider = Provider::new().await;
        let old = f.change().await;
        let c = github_sync_consumer(None);
        assert_eq!(c.delivery_mode(), DeliveryMode::DatabaseAtomic);
        assert_eq!(c.batch_event_cap(), 1);
        let run = dispatcher(&f, None);
        wait_cursor(&f, &old).await;
        run.request_shutdown();
        run.join().await.unwrap();
        assert_eq!(f.protected().await, (1, 1, 0, 0));
        assert!(provider.calls().is_empty());
        sqlx::query("UPDATE tasks SET status_id=?1 WHERE id=?2")
            .bind(f.statuses[0].as_bytes().as_slice())
            .bind(f.task.as_bytes().as_slice())
            .execute(&f.f.pool)
            .await
            .unwrap();
        let new = f.change().await;
        let run = dispatcher(&f, Some(provider.config()));
        wait_cursor(&f, &new).await;
        run.request_shutdown();
        run.join().await.unwrap();
        assert!(!is_processed_backend(&f.f.backend, GITHUB_CONSUMER, old.id)
            .await
            .unwrap());
        assert!(is_processed_backend(&f.f.backend, GITHUB_CONSUMER, new.id)
            .await
            .unwrap());
        assert_eq!(f.protected().await, (1, 1, 1, 0));
        assert_calls(
            &provider.calls(),
            "open",
            "/api/v3/repos/octo/repo/issues/7",
        );
        provider.finish().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn actual_backend_unconfigured_replaced_expired_cancelled_owner_and_two_pool_reuse() {
        let f = TaskFixture::new("octo/repo").await;
        let event = f.change().await;
        let owner = lease(&f, &f.f.backend).await;
        let consumer = github_sync_consumer(None);
        let protected = f.protected().await;
        assert!(consumer
            .deliver_backend(&f.f.backend, Uuid::now_v7(), &event)
            .await
            .is_err());
        assert_eq!(
            f.cursor().await,
            Some(OutboxCursor::SqliteFamily { seq: 0 })
        );
        assert_eq!(f.protected().await, protected);
        sqlx::query("UPDATE outbox_consumers SET lease_until=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)-1 WHERE consumer='github'").execute(&f.f.pool).await.unwrap();
        assert!(consumer
            .deliver_backend(&f.f.backend, owner, &event)
            .await
            .is_err());
        let second = crate::db::pool::connect_sqlite_app(&f.f.dir.join("test.sqlite"), 1)
            .await
            .unwrap();
        let backend = Backend::Sqlite(second.clone());
        let replacement = Uuid::now_v7();
        assert!(
            lease_consumer_backend(&backend, GITHUB_CONSUMER, replacement, 30)
                .await
                .unwrap()
        );
        assert!(consumer
            .deliver_backend(&f.f.backend, owner, &event)
            .await
            .is_err());
        assert_eq!(f.protected().await, protected);
        let hold = backend.begin_write().await.unwrap();
        let waiting = {
            let b = f.f.backend.clone();
            let c = consumer.clone();
            let e = event.clone();
            tokio::spawn(async move { c.deliver_backend(&b, replacement, &e).await })
        };
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        waiting.abort();
        assert!(waiting.await.unwrap_err().is_cancelled());
        hold.rollback().await.unwrap();
        f.f.backend.ping().await.unwrap();
        backend.ping().await.unwrap();
        assert_eq!(
            f.cursor().await,
            Some(OutboxCursor::SqliteFamily { seq: 0 })
        );
        assert_eq!(f.protected().await, protected);
        consumer
            .deliver_backend(&f.f.backend, replacement, &event)
            .await
            .unwrap();
        assert_eq!(f.cursor().await, Some(event.cursor()));
        assert!(consumer
            .deliver_backend(&f.f.backend, replacement, &event)
            .await
            .is_err());
        assert_eq!(f.protected().await, protected);
        assert_eq!(f.cursor().await, Some(event.cursor()));
        second.close().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn actual_backend_configured_reads_current_categories_and_literal_encoded_paths() {
        let f = TaskFixture::new("octo/a?b#c").await;
        let provider = Provider::new().await;
        let consumer = github_sync_consumer(Some(provider.config()));
        assert_eq!(consumer.delivery_mode(), DeliveryMode::External);
        assert_eq!(consumer.batch_event_cap(), 1);
        for (n, want) in [
            (0, "open"),
            (1, "open"),
            (2, "open"),
            (3, "closed"),
            (4, "closed"),
        ] {
            sqlx::query("UPDATE tasks SET status_id=?1 WHERE id=?2")
                .bind(f.statuses[n].as_bytes().as_slice())
                .bind(f.task.as_bytes().as_slice())
                .execute(&f.f.pool)
                .await
                .unwrap();
            let event = f.change().await;
            let target = sync_target_backend(&f.f.backend, &event)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                (
                    target.installation_id.as_str(),
                    target.repo.as_str(),
                    target.issue_number,
                    target.state
                ),
                ("42", "octo/a?b#c", 7, want)
            );
            consumer
                .deliver_backend(&f.f.backend, Uuid::now_v7(), &event)
                .await
                .unwrap();
            let calls = provider.calls();
            assert_calls(
                &calls[n * 2..n * 2 + 2],
                want,
                "/api/v3/repos/octo/a%3Fb%23c/issues/7",
            );
            assert_eq!(f.protected().await, (1, 1, 0, 0));
            assert_eq!(f.cursor().await, None);
        }
        provider.finish().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn actual_backend_configured_missing_deleted_cross_tenant_and_echo_refuse_transport() {
        let f = TaskFixture::new("octo/repo").await;
        let provider = Provider::new().await;
        let consumer = github_sync_consumer(Some(provider.config()));
        for (verb, workspace, target, payload, channel) in [
            (
                "task.created",
                Some(f.f.workspace),
                Some(f.task),
                json!({"to":"ignored"}),
                "web",
            ),
            (
                "task.updated",
                Some(f.f.workspace),
                Some(f.task),
                json!({}),
                "web",
            ),
            (
                "task.updated",
                Some(f.f.workspace),
                Some(f.task),
                json!({"to":null}),
                "web",
            ),
            (
                "task.updated",
                Some(f.f.workspace),
                Some(f.task),
                json!({"to":"ignored"}),
                "webhook",
            ),
            (
                "task.updated",
                Some(f.f.other_workspace),
                Some(f.task),
                json!({"to":"ignored"}),
                "web",
            ),
            (
                "task.updated",
                Some(f.f.workspace),
                Some(Uuid::now_v7()),
                json!({"to":"ignored"}),
                "web",
            ),
            (
                "task.updated",
                Some(f.f.workspace),
                None,
                json!({"to":"ignored"}),
                "web",
            ),
            (
                "task.updated",
                None,
                Some(f.task),
                json!({"to":"ignored"}),
                "web",
            ),
        ] {
            let event = f.event(verb, workspace, target, payload, channel).await;
            consumer
                .deliver_backend(&f.f.backend, Uuid::now_v7(), &event)
                .await
                .unwrap();
            assert!(provider.calls().is_empty());
            assert_eq!(f.protected().await, (1, 1, 0, 0));
            assert_eq!(f.cursor().await, None);
        }
        let event = f.change().await;
        for sql in [
            "UPDATE tasks SET deleted_at=1700000000123456",
            "UPDATE workspaces SET deleted_at=1700000000123456",
        ] {
            sqlx::query(sql).execute(&f.f.pool).await.unwrap();
            consumer
                .deliver_backend(&f.f.backend, Uuid::now_v7(), &event)
                .await
                .unwrap();
            assert!(provider.calls().is_empty());
            assert_eq!(f.protected().await, (1, 1, 0, 0));
            assert_eq!(f.cursor().await, None);
            sqlx::query("UPDATE tasks SET deleted_at=NULL")
                .execute(&f.f.pool)
                .await
                .unwrap();
            sqlx::query("UPDATE workspaces SET deleted_at=NULL")
                .execute(&f.f.pool)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM github_issue_links")
            .execute(&f.f.pool)
            .await
            .unwrap();
        consumer
            .deliver_backend(&f.f.backend, Uuid::now_v7(), &event)
            .await
            .unwrap();
        assert!(provider.calls().is_empty());
        assert_eq!(f.protected().await, (0, 1, 0, 0));
        sqlx::query("INSERT INTO github_issue_links(id,workspace_id,task_id,repo,issue_number) VALUES(?1,?2,?3,'../repo',7)").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.f.workspace.as_bytes().as_slice()).bind(f.task.as_bytes().as_slice()).execute(&f.f.pool).await.unwrap();
        consumer
            .deliver_backend(&f.f.backend, Uuid::now_v7(), &event)
            .await
            .unwrap();
        assert!(provider.calls().is_empty());
        assert_eq!(f.protected().await, (1, 1, 0, 0));
        sqlx::query("UPDATE github_issue_links SET repo='octo/repo'")
            .execute(&f.f.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM github_installations")
            .execute(&f.f.pool)
            .await
            .unwrap();
        consumer
            .deliver_backend(&f.f.backend, Uuid::now_v7(), &event)
            .await
            .unwrap();
        assert!(provider.calls().is_empty());
        assert_eq!(f.protected().await, (1, 0, 0, 0));
        sqlx::query(
            "INSERT INTO github_installations(id,workspace_id,installation_id) VALUES(?1,?2,'42')",
        )
        .bind(Uuid::now_v7().as_bytes().as_slice())
        .bind(f.f.workspace.as_bytes().as_slice())
        .execute(&f.f.pool)
        .await
        .unwrap();
        consumer
            .deliver_backend(&f.f.backend, Uuid::now_v7(), &event)
            .await
            .unwrap();
        assert_calls(
            &provider.calls(),
            "closed",
            "/api/v3/repos/octo/repo/issues/7",
        );
        assert_eq!(f.protected().await, (1, 1, 0, 0));
        let mut tx = f.f.backend.begin_read().await.unwrap();
        tx.operation()
            .set_tenant(f.f.other_workspace)
            .await
            .unwrap();
        assert!(tx
            .operation()
            .github_sync_target(f.f.workspace, f.task)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        assert_eq!(f.protected().await, (1, 1, 0, 0));
        let wrong_fk=sqlx::query("INSERT INTO github_issue_links(id,workspace_id,task_id,repo,issue_number) VALUES(?1,?2,?3,'other/repo',8)").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.f.other_workspace.as_bytes().as_slice()).bind(f.task.as_bytes().as_slice()).execute(&f.f.pool).await;
        assert!(wrong_fk.is_err());
        assert_eq!(f.protected().await, (1, 1, 0, 0));
        provider.finish().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn actual_backend_configured_client_errors_settle_transient_errors_do_not() {
        let f = TaskFixture::new("octo/repo").await;
        let provider = Provider::new().await;
        provider.state.token_status.store(404, Ordering::SeqCst);
        let event = f.change().await;
        let run = dispatcher(&f, Some(provider.config()));
        wait_cursor(&f, &event).await;
        run.request_shutdown();
        run.join().await.unwrap();
        assert_eq!(provider.calls().len(), 1);
        assert_eq!(f.protected().await, (1, 1, 1, 0));
        provider.state.token_status.store(201, Ordering::SeqCst);
        provider.state.patch_status.store(404, Ordering::SeqCst);
        let event = f.change().await;
        let run = dispatcher(&f, Some(provider.config()));
        wait_cursor(&f, &event).await;
        run.request_shutdown();
        run.join().await.unwrap();
        assert_eq!(provider.calls().len(), 3);
        assert_eq!(f.protected().await, (1, 1, 2, 0));
        let prior = f.cursor().await;
        let consumer = github_sync_consumer(Some(provider.config()));
        for (token, patch) in [(500, 200), (201, 503)] {
            provider.state.token_status.store(token, Ordering::SeqCst);
            provider.state.patch_status.store(patch, Ordering::SeqCst);
            let event = f.change().await;
            assert!(consumer
                .deliver_backend(&f.f.backend, Uuid::now_v7(), &event)
                .await
                .is_err());
            assert!(
                !is_processed_backend(&f.f.backend, GITHUB_CONSUMER, event.id)
                    .await
                    .unwrap()
            );
            assert_eq!(f.cursor().await, prior);
            assert_eq!(f.protected().await, (1, 1, 2, 0));
        }
        // A real owned TCP peer closes before a token response: this is a
        // transport error, not an HTTP status or a mock classification.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let reset_peer = tokio::spawn(async move {
            let (connection, _) = listener.accept().await.unwrap();
            drop(connection);
            drop(listener);
        });
        let reset_config = GithubConfig::new(
            "123",
            TEST_KEY,
            "synthetic-secret",
            &format!("http://{address}"),
            [7; 32],
        )
        .unwrap();
        let event = f.change().await;
        assert!(github_sync_consumer(Some(reset_config))
            .deliver_backend(&f.f.backend, Uuid::now_v7(), &event)
            .await
            .is_err());
        reset_peer.await.unwrap();
        assert!(
            !is_processed_backend(&f.f.backend, GITHUB_CONSUMER, event.id)
                .await
                .unwrap()
        );
        assert_eq!(f.cursor().await, prior);
        assert_eq!(f.protected().await, (1, 1, 2, 0));
        provider.state.token_status.store(201, Ordering::SeqCst);
        provider.state.patch_status.store(200, Ordering::SeqCst);
        let run = dispatcher(&f, Some(provider.config()));
        let last: BackendOutboxEvent = {
            let id: Vec<u8> = sqlx::query_scalar("SELECT id FROM events ORDER BY seq DESC LIMIT 1")
                .fetch_one(&f.f.pool)
                .await
                .unwrap();
            fetch_event_by_id_backend(&f.f.backend, Uuid::from_slice(&id).unwrap())
                .await
                .unwrap()
                .unwrap()
        };
        wait_cursor(&f, &last).await;
        run.request_shutdown();
        run.join().await.unwrap();
        assert_eq!(f.protected().await, (1, 1, 5, 0));
        provider.finish().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn actual_backend_confirmed_external_effect_marks_before_replaced_owner_cursor() {
        let f = TaskFixture::new("octo/repo").await;
        let provider = Provider::new().await;
        provider.state.hold_patch.store(true, Ordering::SeqCst);
        let event = f.change().await;
        let run = dispatcher(&f, Some(provider.config()));
        tokio::time::timeout(Duration::from_secs(3), provider.state.patch_seen.notified())
            .await
            .unwrap();
        // Network is in flight, yet an independent real writer can take the database.
        let second = crate::db::pool::connect_sqlite_app(&f.f.dir.join("test.sqlite"), 1)
            .await
            .unwrap();
        let backend = Backend::Sqlite(second.clone());
        let writer = backend.begin_write().await.unwrap();
        writer.rollback().await.unwrap();
        sqlx::query("UPDATE outbox_consumers SET lease_until=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)-1 WHERE consumer='github'").execute(&second).await.unwrap();
        let replacement = Uuid::now_v7();
        assert!(
            lease_consumer_backend(&backend, GITHUB_CONSUMER, replacement, 30)
                .await
                .unwrap()
        );
        run.request_shutdown();
        provider.state.release_patch.notify_one();
        run.join().await.unwrap();
        wait_mark(&f, &event).await;
        assert_eq!(
            f.cursor().await,
            Some(OutboxCursor::SqliteFamily { seq: 0 })
        );
        assert_eq!(f.protected().await, (1, 1, 1, 0));
        assert_calls(
            &provider.calls(),
            "closed",
            "/api/v3/repos/octo/repo/issues/7",
        );
        assert!(
            release_consumer_backend(&backend, GITHUB_CONSUMER, replacement)
                .await
                .unwrap()
        );
        let run = dispatcher(&f, Some(provider.config()));
        wait_cursor(&f, &event).await;
        run.request_shutdown();
        run.join().await.unwrap();
        assert_eq!(provider.calls().len(), 2);
        assert_eq!(f.protected().await, (1, 1, 1, 0));
        second.close().await;
        provider.finish().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn actual_backend_cancelled_unknown_external_response_remains_unmarked_until_restart() {
        let f = TaskFixture::new("octo/repo").await;
        let provider = Provider::new().await;
        provider.state.hold_patch.store(true, Ordering::SeqCst);
        let event = f.change().await;
        let waiting = {
            let b = f.f.backend.clone();
            let c = github_sync_consumer(Some(provider.config()));
            let e = event.clone();
            tokio::spawn(async move { c.deliver_backend(&b, Uuid::now_v7(), &e).await })
        };
        tokio::time::timeout(Duration::from_secs(3), provider.state.patch_seen.notified())
            .await
            .unwrap();
        waiting.abort();
        assert!(waiting.await.unwrap_err().is_cancelled());
        provider.state.release_patch.notify_one();
        f.f.backend.ping().await.unwrap();
        assert_eq!(f.protected().await, (1, 1, 0, 0));
        assert_eq!(f.cursor().await, None);
        let run = dispatcher(&f, Some(provider.config()));
        wait_cursor(&f, &event).await;
        run.request_shutdown();
        run.join().await.unwrap();
        let calls = provider.calls();
        assert_eq!(calls.len(), 4);
        assert_calls(&calls[0..2], "closed", "/api/v3/repos/octo/repo/issues/7");
        assert_calls(&calls[2..4], "closed", "/api/v3/repos/octo/repo/issues/7");
        assert_eq!(f.protected().await, (1, 1, 1, 0));
        provider.finish().await;
        f.finish().await;
    }
}
