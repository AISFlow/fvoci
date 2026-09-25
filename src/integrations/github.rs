//! GitHub App integration (source `core/github.ts`).
//!
//! Workspace admins install the app (signed `state` round trip through
//! github.com), link tasks to issues, and receive signed GitHub webhooks that
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
use ring::rand::SystemRandom;
use ring::signature::{RsaKeyPair, RSA_PKCS1_SHA256};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::{PgPool, Postgres, Transaction};
use tracing::warn;
use url::Url;
use uuid::Uuid;

use crate::db::context::{
    lock_membership_users, recheck_session, restore_system, set_system, set_tenant,
};
use crate::db::documents::{
    between, empty_document_json, workspace_is_live, DOCUMENT_SCHEMA_VERSION,
};
use crate::db::identity::{append_audit, append_event_channel, AuditAppend, EventAppend};
use crate::db::integrations::{require_manager_read, require_manager_write, IntegrationDbError};
use crate::db::outbox::OutboxEvent;
use crate::db::projects::{lock_project, project_permission};
use crate::db::task_activity::record_task_activity;
use crate::outbox::{DeliveryMode, OutboxConsumer, OutboxProcessError};
use crate::projects::ProjectPermission;
use crate::tasks::activity::ActivitySnapshot;

pub const GITHUB_CONSUMER: &str = "github";
pub const GITHUB_API_DEFAULT: &str = "https://api.github.com";
const GITHUB_WEB: &str = "https://github.com";
const STATE_TTL_MS: i64 = 600_000;
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
    pub api_base: Url,
}

impl std::fmt::Debug for GithubConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubConfig")
            .field("app_id", &self.app_id)
            .field("private_key", &"<redacted>")
            .field("webhook_secret", &"<redacted>")
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

impl GithubConfig {
    pub fn new(
        app_id: &str,
        private_key_pem: &str,
        webhook_secret: &str,
        api_base: &str,
    ) -> Result<Self, String> {
        let app_id = app_id.trim();
        if app_id.is_empty() || webhook_secret.is_empty() {
            return Err("GITHUB_APP_ID and GITHUB_WEBHOOK_SECRET must be non-empty".into());
        }
        let api_base = Url::parse(api_base.trim().trim_end_matches('/'))
            .map_err(|err| format!("invalid GITHUB_API_URL: {err}"))?;
        if !matches!(api_base.scheme(), "http" | "https") {
            return Err("GITHUB_API_URL must be http(s)".into());
        }
        Ok(Self {
            app_id: app_id.to_string(),
            key: Arc::new(parse_private_key(private_key_pem)?),
            webhook_secret: webhook_secret.to_string(),
            api_base,
        })
    }

    /// `GITHUB_APP_ID`, `GITHUB_APP_PRIVATE_KEY`, `GITHUB_WEBHOOK_SECRET` all
    /// set, or none (source superRefine).
    pub fn from_env() -> Result<Option<Self>, String> {
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
                Self::new(&id, &key, &secret, &api).map(Some)
            }
            _ => Err(
                "GITHUB_APP_ID, GITHUB_APP_PRIVATE_KEY, GITHUB_WEBHOOK_SECRET must all be set together, or none"
                    .into(),
            ),
        }
    }

    fn api(&self, path: &str) -> String {
        format!("{}{}", self.api_base.as_str().trim_end_matches('/'), path)
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

    /// Install `state` MAC key. Source uses SECRET_KEY; this server has no
    /// general secret, so the key is derived from the app webhook secret with a
    /// distinct label (the webhook MAC itself signs only GitHub bodies).
    fn state_key(&self) -> [u8; 32] {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(self.webhook_secret.as_bytes()).expect("hmac key");
        mac.update(b"fvoci:github-install-state:v1");
        mac.finalize().into_bytes().into()
    }

    pub fn sign_install_state(&self, workspace_id: Uuid, now_ms: i64) -> String {
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(
                &json!({ "w": workspace_id.to_string(), "e": now_ms + STATE_TTL_MS }),
            )
            .expect("state json"),
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.state_key()).expect("hmac key");
        mac.update(payload.as_bytes());
        format!("{payload}.{}", hex::encode(mac.finalize().into_bytes()))
    }

    pub fn verify_install_state(&self, state: &str, now_ms: i64) -> Option<Uuid> {
        let (payload, mac_hex) = state.rsplit_once('.')?;
        if payload.is_empty()
            || mac_hex.len() != 64
            || !mac_hex
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return None;
        }
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.state_key()).expect("hmac key");
        mac.update(payload.as_bytes());
        mac.verify_slice(&hex::decode(mac_hex).ok()?).ok()?;
        let decoded: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
        let workspace_id = Uuid::parse_str(decoded.get("w")?.as_str()?).ok()?;
        let expires = decoded.get("e")?.as_f64()?;
        if expires < now_ms as f64 {
            return None;
        }
        Some(workspace_id)
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
    github: &GithubConfig,
    method: reqwest::Method,
    path: &str,
    bearer: &str,
    body: Option<Value>,
) -> Result<(u16, Option<Value>), GithubApiError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(GITHUB_REQUEST_TIMEOUT)
        .build()
        .map_err(|err| GithubApiError::Transport(err.without_url().to_string()))?;
    let mut request = client
        .request(method, github.api(path))
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
    let (status, body) = github_call(github, reqwest::Method::GET, "/app", &jwt, None).await?;
    if !(200..300).contains(&status) {
        return Err(GithubApiError::Status(status));
    }
    string_field(&body, "slug").ok_or(GithubApiError::Invalid)
}

/// Source `installationToken`.
async fn installation_token(
    github: &GithubConfig,
    installation_id: &str,
) -> Result<String, GithubApiError> {
    let jwt = github.app_jwt().map_err(|_| GithubApiError::Invalid)?;
    let (status, body) = github_call(
        github,
        reqwest::Method::POST,
        &format!("/app/installations/{installation_id}/access_tokens"),
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

/// Source `completeGithubInstall`: the verified state names the workspace;
/// one installation per workspace, one workspace per installation.
pub async fn complete_install(
    pool: &PgPool,
    workspace_id: Uuid,
    installation_id: &str,
) -> Result<Result<(), IntegrationDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    }
    let result = sqlx::query(
        r#"
        INSERT INTO fvoci.github_installations (id, workspace_id, installation_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (workspace_id)
        DO UPDATE SET installation_id = EXCLUDED.installation_id, updated_at = now()
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
            actor_user_id: None,
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

pub async fn sync_linked_issue(
    pool: &PgPool,
    github: &GithubConfig,
    event: &OutboxEvent,
) -> Result<(), OutboxProcessError> {
    let Some(target) = sync_target(pool, event).await? else {
        return Ok(());
    };
    let token = installation_token(github, &target.installation_id)
        .await
        .map_err(|err| OutboxProcessError::Delivery(format!("github token: {err}")))?;
    let (status, _) = github_call(
        github,
        reqwest::Method::PATCH,
        &format!("/repos/{}/issues/{}", target.repo, target.issue_number),
        &token,
        Some(json!({ "state": target.state })),
    )
    .await
    .map_err(|err| OutboxProcessError::Delivery(format!("github patch: {err}")))?;
    if (400..500).contains(&status) {
        // Source: a client error is final (not retried).
        warn!(event_id = %event.id, http_status = status, "github.request_failed");
        return Ok(());
    }
    if !(200..300).contains(&status) {
        return Err(OutboxProcessError::Delivery(format!(
            "github patch status {status}"
        )));
    }
    Ok(())
}

pub struct GithubSyncConsumer {
    github: GithubConfig,
}

pub fn github_sync_consumer(github: GithubConfig) -> Arc<dyn OutboxConsumer> {
    Arc::new(GithubSyncConsumer { github })
}

impl OutboxConsumer for GithubSyncConsumer {
    fn name(&self) -> &str {
        GITHUB_CONSUMER
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::External
    }

    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        _lease_owner: Uuid,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move { sync_linked_issue(pool, &self.github, event).await })
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
        GithubConfig::new("123", TEST_KEY, "whsec", "http://127.0.0.1:9").expect("config")
    }

    #[test]
    fn state_round_trip_expiry_and_tamper() {
        let github = config();
        let ws = Uuid::now_v7();
        let state = github.sign_install_state(ws, 1_000);
        assert_eq!(github.verify_install_state(&state, 1_000), Some(ws));
        assert_eq!(
            github.verify_install_state(&state, 1_000 + STATE_TTL_MS + 1),
            None
        );
        let mut tampered = state.clone();
        tampered.insert(0, 'x');
        assert_eq!(github.verify_install_state(&tampered, 1_000), None);
        let other = GithubConfig::new("123", TEST_KEY, "other", "http://127.0.0.1:9").expect("c");
        assert_eq!(other.verify_install_state(&state, 1_000), None);
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
        assert!(GithubConfig::new("1", "not a key", "s", GITHUB_API_DEFAULT).is_err());
    }
}
