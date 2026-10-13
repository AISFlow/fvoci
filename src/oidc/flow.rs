//! Source `beginOidcFlow` / `completeOidcFlow` and the three modes
//! (`login`, `link`, `invite`).
//!
//! The flow state (nonce, PKCE verifier, mode, bindings) is a single-use
//! sealed row keyed by the state's hash; the browser holds
//! `<state>.<hmac>` in an HttpOnly cookie so the callback must come back to
//! the browser that started it (login CSRF). Every sign-in result goes
//! through the MFA gate.

use std::collections::HashMap;

use crate::db::backend::Backend;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{Duration, Utc};
use rand::Rng;
use ring::hmac;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::password::Keyring;
use crate::auth::token::hash_token;
use crate::db::invitations::{self, IdentityAcceptRequest, InvitationDbError};
use crate::db::mfa::{issue_session_or_challenge_backend, IssueOptions, Issued};
use crate::db::oidc::{self as db, JitInput, JitOutcome, LinkOutcome, NewLink};
use crate::identity::{workspace_oidc_context, Identity};
use crate::oidc::client::{self, CodeExchange, SocialProfile};
use crate::oidc::providers::{ProviderKey, ProviderKind, ResolvedProvider};
use crate::secret_box;

pub const STATE_COOKIE: &str = "fvoci_oidc_state";
pub const STATE_TTL_SECS: i64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Login,
    Link,
    Invite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OidcErrorCode {
    NotLinked,
    StateMismatch,
    ProviderError,
    AlreadyLinked,
    InvitationInvalid,
}

impl OidcErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotLinked => "oidc_not_linked",
            Self::StateMismatch => "oidc_state_mismatch",
            Self::ProviderError => "oidc_provider_error",
            Self::AlreadyLinked => "oidc_already_linked",
            Self::InvitationInvalid => "oidc_invitation_invalid",
        }
    }
}

#[derive(Debug)]
pub enum OidcResult {
    Session {
        user_id: Uuid,
        token: String,
    },
    Mfa {
        mfa_token: String,
    },
    Linked,
    Error {
        code: OidcErrorCode,
        mode: Option<Mode>,
    },
    /// Source rethrows `QuotaExceededError` (402) from the callback.
    SeatLimit,
    /// Link mode: the session that started the link was revoked, expired or
    /// its account closed before the link was saved (401, like unlink).
    SessionGone,
}

fn error(code: OidcErrorCode, mode: Option<Mode>) -> OidcResult {
    OidcResult::Error { code, mode }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredState {
    provider: String,
    mode: Mode,
    nonce: String,
    pkce_verifier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    invitation_token_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    consents: Option<Vec<(String, i32)>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace_id: Option<Uuid>,
    expires_at_ms: i64,
    issuer: String,
    client_id: String,
}

fn random_b64(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut raw);
    URL_SAFE_NO_PAD.encode(raw)
}

fn state_context(state_hash: &str) -> String {
    format!("oidc-state:{state_hash}")
}

/// Cookie MAC key derived from an encryption key (the source signs with its
/// SECRET_KEY, which this server does not have).
fn cookie_key(material: &[u8]) -> hmac::Key {
    let root = hmac::Key::new(hmac::HMAC_SHA256, material);
    let derived = hmac::sign(&root, b"fvoci:oidc-state-cookie:v1");
    hmac::Key::new(hmac::HMAC_SHA256, derived.as_ref())
}

pub fn sign_state(ring: &Keyring, state: &str) -> Option<String> {
    let material = ring.keys.get(&ring.active_id)?;
    let tag = hmac::sign(&cookie_key(material), state.as_bytes());
    Some(format!("{state}.{}", hex::encode(tag.as_ref())))
}

/// Source `verifySignedState`: the cookie names exactly this state and its
/// MAC verifies under one of the keys (rotation keeps in-flight flows).
pub fn verify_signed_state(ring: &Keyring, state: &str, signed: Option<&str>) -> bool {
    let Some(signed) = signed else {
        return false;
    };
    let Some((cookie_state, mac_hex)) = signed.rsplit_once('.') else {
        return false;
    };
    if cookie_state.is_empty()
        || !crate::auth::token::token_hashes_eq(cookie_state, state)
        || mac_hex.len() != 64
    {
        return false;
    }
    let Ok(mac) = hex::decode(mac_hex) else {
        return false;
    };
    ring.keys
        .values()
        .any(|material| hmac::verify(&cookie_key(material), state.as_bytes(), &mac).is_ok())
}

pub struct BeginParams {
    pub provider: ProviderKey,
    pub mode: Mode,
    pub invitation_token: Option<String>,
    pub user_id: Option<Uuid>,
    pub consents: Vec<(String, i32)>,
    pub workspace_id: Option<Uuid>,
}

pub struct Started {
    pub authorization_url: String,
    pub signed_state: String,
}

#[derive(Debug)]
pub enum BeginError {
    NotConfigured,
    Unavailable,
    Provider(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for BeginError {
    fn from(err: sqlx::Error) -> Self {
        Self::Db(err)
    }
}

fn workspace_provider(keys: &Keyring, row: &db::WorkspaceOidcRow) -> Option<ResolvedProvider> {
    let secret = secret_box::open(
        keys,
        &row.client_secret,
        &workspace_oidc_context(row.workspace_id),
    )
    .map_err(|err| tracing::error!(error = %err, "workspace oidc secret does not open"))
    .ok()?;
    Some(ResolvedProvider {
        key: ProviderKey::Generic,
        label: row.label.clone(),
        issuer: row.issuer.clone(),
        client_id: row.client_id.clone(),
        client_secret: secret,
        kind: ProviderKind::Oidc,
        scope: "openid email profile",
        microsoft_tenant: None,
    })
}

/// Workspace SSO answers on the workspace's own callback, instance providers
/// on theirs (see [`crate::oidc::OidcSettings::workspace_redirect_uri`]).
fn flow_redirect_uri(
    settings: &crate::oidc::OidcSettings,
    key: ProviderKey,
    workspace_id: Option<Uuid>,
) -> String {
    match workspace_id {
        Some(workspace_id) => settings.workspace_redirect_uri(workspace_id),
        None => settings.redirect_uri(key),
    }
}

async fn load_workspace_provider(
    pool: &Backend,
    keys: &Keyring,
    workspace_id: Uuid,
) -> Result<Option<ResolvedProvider>, sqlx::Error> {
    Ok(db::workspace_oidc_for_sign_in_backend(pool, workspace_id)
        .await?
        .and_then(|row| workspace_provider(keys, &row)))
}

pub async fn begin_backend(
    pool: &Backend,
    identity: &Identity,
    license: &crate::license::Entitlements,
    params: BeginParams,
) -> Result<Started, BeginError> {
    let keys = identity
        .encryption_keys
        .as_deref()
        .ok_or(BeginError::Unavailable)?;
    let settings = &identity.oidc;
    let workspace_sso = matches!(params.mode, Mode::Login | Mode::Link)
        && params.provider == ProviderKey::Generic
        && params.workspace_id.is_some();
    if workspace_sso && !license.has_feature("workspaceSso") {
        return Err(BeginError::NotConfigured);
    }
    let workspace_id = workspace_sso.then_some(params.workspace_id).flatten();
    let provider = match workspace_id {
        Some(workspace_id) => load_workspace_provider(pool, keys, workspace_id).await?,
        None => settings.find(params.provider).cloned(),
    }
    .ok_or(BeginError::NotConfigured)?;

    let state = random_b64(32);
    let nonce = random_b64(32);
    let verifier = random_b64(32);
    let redirect_uri = flow_redirect_uri(settings, provider.key, workspace_id);
    let authorization_url = match provider.kind {
        ProviderKind::OAuth2Naver => {
            let mut url = url::Url::parse(&format!("{}/oauth2.0/authorize", provider.issuer))
                .map_err(|_| BeginError::Provider("naver issuer".into()))?;
            url.query_pairs_mut()
                .append_pair("response_type", "code")
                .append_pair("client_id", &provider.client_id)
                .append_pair("redirect_uri", &redirect_uri)
                .append_pair("state", &state);
            url.to_string()
        }
        ProviderKind::Oidc => {
            let discovery = settings
                .cache
                .discovery(settings.fetch_policy(), &provider)
                .await
                .map_err(|err| BeginError::Provider(err.to_string()))?;
            client::authorization_url(
                &discovery,
                &provider,
                &redirect_uri,
                &state,
                &nonce,
                &verifier,
            )
            .map_err(|err| BeginError::Provider(err.to_string()))?
        }
    };

    let now = Utc::now();
    let stored = StoredState {
        provider: provider.key.as_str().to_string(),
        mode: params.mode,
        nonce,
        pkce_verifier: verifier,
        invitation_token_hash: params.invitation_token.as_deref().map(hash_token),
        user_id: params.user_id,
        consents: (!params.consents.is_empty()).then_some(params.consents),
        workspace_id,
        expires_at_ms: (now + Duration::seconds(STATE_TTL_SECS)).timestamp_millis(),
        issuer: provider.issuer.clone(),
        client_id: provider.client_id.clone(),
    };
    let state_hash = hash_token(&state);
    let payload = serde_json::to_string(&stored).expect("state json");
    let sealed = secret_box::seal(keys, &payload, &state_context(&state_hash))
        .map_err(|_| BeginError::Unavailable)?;
    db::issue_state_backend(
        pool,
        &state_hash,
        &sealed,
        now + Duration::seconds(STATE_TTL_SECS),
    )
    .await?;
    let signed_state = sign_state(keys, &state).ok_or(BeginError::Unavailable)?;
    Ok(Started {
        authorization_url,
        signed_state,
    })
}

/// Tenant SSO stores the subject as `<workspace_id>:<sub>`.
fn identity_subject(provider: ProviderKey, sub: &str, workspace_id: Option<Uuid>) -> String {
    match (provider, workspace_id) {
        (ProviderKey::Generic, Some(ws)) => format!("{ws}:{sub}"),
        _ => sub.to_string(),
    }
}

async fn oidc_session(
    pool: &Backend,
    user_id: Uuid,
    provider: ProviderKey,
) -> Result<OidcResult, sqlx::Error> {
    let method = format!("oidc:{}", provider.as_str());
    Ok(
        match issue_session_or_challenge_backend(pool, user_id, &method, IssueOptions::default())
            .await?
        {
            Some(Issued::Session { user_id, token }) => OidcResult::Session { user_id, token },
            Some(Issued::Challenge { mfa_token }) => OidcResult::Mfa { mfa_token },
            None => error(OidcErrorCode::ProviderError, None),
        },
    )
}

pub struct CompleteParams<'a> {
    pub provider: ProviderKey,
    /// The workspace of the callback route: `Some` on
    /// `/api/v1/auth/sso/{workspace}/callback`, `None` on an instance
    /// provider's `/api/v1/auth/oidc/{provider}/callback`.
    pub workspace_id: Option<Uuid>,
    pub query: &'a HashMap<String, String>,
    pub signed_state: Option<&'a str>,
    /// Live session cookie at callback start: (user id, session id).
    pub session: Option<(Uuid, Uuid)>,
    pub ip: Option<&'a str>,
    pub defaults: &'a crate::settings::DefaultsUserSettings,
}

pub async fn complete_backend(
    pool: &Backend,
    identity: &Identity,
    license: &crate::license::Entitlements,
    params: CompleteParams<'_>,
) -> Result<OidcResult, sqlx::Error> {
    let Some(keys) = identity.encryption_keys.as_deref() else {
        return Ok(error(OidcErrorCode::StateMismatch, None));
    };
    let settings = &identity.oidc;
    let Some(state) = params.query.get("state").filter(|s| !s.is_empty()) else {
        return Ok(error(OidcErrorCode::StateMismatch, None));
    };
    if !verify_signed_state(keys, state, params.signed_state) {
        return Ok(error(OidcErrorCode::StateMismatch, None));
    }
    let state_hash = hash_token(state);
    let Some(sealed) = db::consume_state_backend(pool, &state_hash).await? else {
        return Ok(error(OidcErrorCode::StateMismatch, None));
    };
    let stored: StoredState = match secret_box::open(keys, &sealed, &state_context(&state_hash))
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
    {
        Some(stored) => stored,
        None => return Ok(error(OidcErrorCode::StateMismatch, None)),
    };
    if stored.expires_at_ms <= Utc::now().timestamp_millis() {
        return Ok(error(OidcErrorCode::StateMismatch, None));
    }
    let mode = Some(stored.mode);
    if stored.provider != params.provider.as_str() {
        return Ok(error(OidcErrorCode::StateMismatch, mode));
    }
    // A response delivered to another flow's redirect URI came from another
    // authorization server (IdP mix-up): refuse it before any discovery or
    // token request can hand the code to the server of this state.
    if stored.workspace_id != params.workspace_id {
        return Ok(error(OidcErrorCode::StateMismatch, mode));
    }
    // A valid state may outlive the license; never exchange it into a session
    // or an identity link after workspace SSO expires.
    if stored.workspace_id.is_some() && !license.has_feature("workspaceSso") {
        return Ok(error(OidcErrorCode::ProviderError, mode));
    }
    if params.query.contains_key("error") {
        return Ok(error(OidcErrorCode::ProviderError, mode));
    }
    let provider = match stored.workspace_id {
        Some(ws) => load_workspace_provider(pool, keys, ws).await?,
        None => settings.find(params.provider).cloned(),
    };
    let Some(provider) =
        provider.filter(|p| p.issuer == stored.issuer && p.client_id == stored.client_id)
    else {
        return Ok(error(OidcErrorCode::ProviderError, mode));
    };
    let Some(code) = params.query.get("code").filter(|c| !c.is_empty()) else {
        return Ok(error(OidcErrorCode::ProviderError, mode));
    };
    let redirect_uri = flow_redirect_uri(settings, provider.key, stored.workspace_id);
    let exchanged = match provider.kind {
        ProviderKind::OAuth2Naver => {
            client::naver_exchange(
                settings.fetch_policy(),
                &provider,
                code,
                state,
                &redirect_uri,
            )
            .await
        }
        ProviderKind::Oidc => {
            client::oidc_exchange(
                &settings.cache,
                settings.fetch_policy(),
                &provider,
                CodeExchange {
                    code,
                    redirect_uri: &redirect_uri,
                    pkce_verifier: &stored.pkce_verifier,
                    nonce: &stored.nonce,
                    iss_param: params.query.get("iss").map(String::as_str),
                    now: Utc::now(),
                },
            )
            .await
        }
    };
    let profile = match exchanged {
        Ok(profile) => profile,
        Err(err) => {
            // No token, code or claim values are logged.
            tracing::warn!(provider = provider.key.as_str(), reason = %err, "oidc.exchange_failed");
            return Ok(error(OidcErrorCode::ProviderError, mode));
        }
    };
    if stored.workspace_id.is_some() && !license.has_feature("workspaceSso") {
        return Ok(error(OidcErrorCode::ProviderError, mode));
    }
    match stored.mode {
        Mode::Login => {
            login_with_identity(pool, license, provider.key, &profile, stored.workspace_id).await
        }
        Mode::Link => link_identity(pool, provider.key, &profile, &stored, params.session).await,
        Mode::Invite => {
            accept_invite_with_identity(
                pool,
                license,
                provider.key,
                &profile,
                &stored,
                params.ip,
                params.defaults,
            )
            .await
        }
    }
}

fn email_domain(email: &str) -> Option<String> {
    let at = email.rfind('@')?;
    if at == 0 || at == email.len() - 1 {
        return None;
    }
    Some(email[at + 1..].to_ascii_lowercase())
}

/// Source `tryJitJoin`. `None` falls back to `oidc_not_linked`.
async fn try_jit_join(
    pool: &Backend,
    license: &crate::license::Entitlements,
    profile: &SocialProfile,
    workspace_id: Uuid,
) -> Result<Option<OidcResult>, sqlx::Error> {
    let Some(email) = profile.email.as_deref() else {
        return Ok(None);
    };
    if !profile.email_verified {
        return Ok(None);
    }
    let Some(domain) = email_domain(email) else {
        return Ok(None);
    };
    let Some(domains) = db::workspace_auto_join_domains_backend(pool, workspace_id).await? else {
        return Ok(None);
    };
    if !db::domain_allowed(&domains, &domain) {
        return Ok(None);
    }
    if crate::db::account::email_exists_backend(pool, email).await? {
        return Ok(None);
    }
    let given_name = profile
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(|n| n.chars().take(100).collect::<String>())
        .unwrap_or_else(|| invitations::email_local_part(email));
    let subject = identity_subject(ProviderKey::Generic, &profile.sub, Some(workspace_id));
    match db::jit_join_backend(
        pool,
        license,
        JitInput {
            workspace_id,
            email,
            domain: &domain,
            given_name: &given_name,
            subject: &subject,
            issuer: &profile.issuer,
        },
    )
    .await?
    {
        JitOutcome::Joined(user_id) => Ok(Some(
            oidc_session(pool, user_id, ProviderKey::Generic).await?,
        )),
        JitOutcome::Skipped => Ok(None),
        JitOutcome::SeatLimit => Ok(Some(OidcResult::SeatLimit)),
    }
}

async fn login_with_identity(
    pool: &Backend,
    license: &crate::license::Entitlements,
    provider: ProviderKey,
    profile: &SocialProfile,
    workspace_id: Option<Uuid>,
) -> Result<OidcResult, sqlx::Error> {
    let subject = identity_subject(provider, &profile.sub, workspace_id);
    let lookup = db::find_link_backend(pool, provider.as_str(), &subject, &profile.issuer).await?;
    // A link made through another issuer is not this identity, and its
    // subject stays taken, so JIT is not tried either.
    let taken = lookup.subject_taken();
    let Some(link) = lookup.found() else {
        if let (ProviderKey::Generic, Some(ws), false) = (provider, workspace_id, taken) {
            if let Some(result) = try_jit_join(pool, license, profile, ws).await? {
                return Ok(result);
            }
        }
        return Ok(error(OidcErrorCode::NotLinked, Some(Mode::Login)));
    };
    oidc_session(pool, link.user_id, provider).await
}

async fn link_identity(
    pool: &Backend,
    provider: ProviderKey,
    profile: &SocialProfile,
    stored: &StoredState,
    session: Option<(Uuid, Uuid)>,
) -> Result<OidcResult, sqlx::Error> {
    let (Some(expected), Some((actual, session_id))) = (stored.user_id, session) else {
        return Ok(error(OidcErrorCode::StateMismatch, Some(Mode::Link)));
    };
    if expected != actual {
        return Ok(error(OidcErrorCode::StateMismatch, Some(Mode::Link)));
    }
    let subject = identity_subject(provider, &profile.sub, stored.workspace_id);
    let outcome = db::link_for_user_backend(
        pool,
        session_id,
        &NewLink {
            user_id: expected,
            provider: provider.as_str(),
            subject: &subject,
            issuer: &profile.issuer,
            email: profile.email.as_deref(),
            workspace_id: None,
        },
    )
    .await?;
    Ok(match outcome {
        LinkOutcome::Linked => OidcResult::Linked,
        LinkOutcome::AlreadyLinked => error(OidcErrorCode::AlreadyLinked, Some(Mode::Link)),
        LinkOutcome::SessionGone => OidcResult::SessionGone,
    })
}

async fn accept_invite_with_identity(
    pool: &Backend,
    license: &crate::license::Entitlements,
    provider: ProviderKey,
    profile: &SocialProfile,
    stored: &StoredState,
    ip: Option<&str>,
    defaults: &crate::settings::DefaultsUserSettings,
) -> Result<OidcResult, sqlx::Error> {
    let invalid = || error(OidcErrorCode::InvitationInvalid, Some(Mode::Invite));
    let Some(token_hash) = stored.invitation_token_hash.as_deref() else {
        return Ok(invalid());
    };
    let consents = stored.consents.clone().unwrap_or_default();
    let subject = identity_subject(provider, &profile.sub, None);
    let accepted = invitations::accept_invitation_with_identity_backend(
        pool,
        license,
        IdentityAcceptRequest {
            token_hash,
            provider: provider.as_str(),
            subject: &subject,
            issuer: &profile.issuer,
            link_email: profile.email.as_deref(),
            given_name: profile.name.as_deref(),
            client_ip: ip,
            consents: &consents,
            defaults,
        },
    )
    .await?;
    match accepted {
        Ok(user_id) => oidc_session(pool, user_id, provider).await,
        Err(InvitationDbError::AlreadyLinked) => {
            Ok(error(OidcErrorCode::AlreadyLinked, Some(Mode::Invite)))
        }
        Err(InvitationDbError::SeatLimit) => Ok(OidcResult::SeatLimit),
        Err(_) => Ok(invalid()),
    }
}

pub async fn begin(
    pool: &PgPool,
    identity: &Identity,
    license: &crate::license::Entitlements,
    params: BeginParams,
) -> Result<Started, BeginError> {
    begin_backend(&Backend::Postgres(pool.clone()), identity, license, params).await
}

pub async fn complete(
    pool: &PgPool,
    identity: &Identity,
    license: &crate::license::Entitlements,
    params: CompleteParams<'_>,
) -> Result<OidcResult, sqlx::Error> {
    complete_backend(&Backend::Postgres(pool.clone()), identity, license, params).await
}

#[cfg(all(test, feature = "db-tests"))]
mod remote_invitation_tests {
    use super::*;
    use crate::db::backend::{DbTransaction, RemoteDatabase};
    use crate::db::codec::Cell;
    use futures_util::FutureExt;

    fn state(hash: &str) -> StoredState {
        StoredState {
            provider: "generic".into(),
            mode: Mode::Invite,
            nonce: "fixture-nonce".into(),
            pkce_verifier: "fixture-verifier".into(),
            invitation_token_hash: Some(hash.into()),
            user_id: None,
            consents: Some(vec![]),
            workspace_id: None,
            expires_at_ms: (Utc::now() + Duration::minutes(1)).timestamp_millis(),
            issuer: "https://idp.example.invalid".into(),
            client_id: "fixture-client".into(),
        }
    }

    fn profile(subject: &str) -> SocialProfile {
        // The HTTP integration test verifies the SDK/provider round trip;
        // this fixture exercises the callback's already-validated identity.
        SocialProfile {
            sub: subject.into(),
            issuer: "https://idp.example.invalid".into(),
            email: Some("remote-idp@example.invalid".into()),
            name: Some("Remote invite".into()),
            email_verified: true,
        }
    }

    async fn effects(backend: &Backend) -> Vec<i64> {
        let mut tx = backend.begin_read().await.unwrap();
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            unreachable!()
        };
        let rows = family
            .query(
                "SELECT (SELECT count(*) FROM users),(SELECT count(*) FROM memberships),\
             (SELECT count(*) FROM identity_links),(SELECT count(*) FROM events),\
             (SELECT count(*) FROM audit_log),(SELECT count(*) FROM sessions),\
             (SELECT count(*) FROM invitations WHERE accepted_at IS NOT NULL)",
                &[],
            )
            .await
            .unwrap();
        let counts = (0..7)
            .map(|i| rows[0].cell(i).unwrap().integer().unwrap())
            .collect();
        tx.rollback().await.unwrap();
        counts
    }

    #[tokio::test]
    #[ignore = "explicit pinned official sqld fixture via FVOCI_TEST_SQLD"]
    async fn remote_oidc_invitation_callback_commits_once_and_refusals_have_no_effect() {
        let sqld = std::path::PathBuf::from(
            std::env::var_os("FVOCI_TEST_SQLD").expect("pinned official sqld required"),
        );
        let root =
            std::env::temp_dir().join(format!("fvoci-remote-oidc-invite-{}", Uuid::now_v7()));
        let driver = crate::db::libsql_finish_fixture::LibsqlFinishFixture::start(&sqld, &root)
            .await
            .unwrap();
        let backend = Backend::LibsqlRemote(std::sync::Arc::new(RemoteDatabase::from_test_driver(
            driver.database().await.unwrap(),
            std::num::NonZeroU32::MIN,
        )));
        let result = std::panic::AssertUnwindSafe(async {
            crate::db::migrate::initialize_family_backend_for_test(&backend).await.unwrap();
            let owner = Uuid::now_v7();
            let workspace = Uuid::now_v7();
            let mut tx = backend.begin_write().await.unwrap();
            let DbTransaction::SqliteFamily(family) = &mut tx else { unreachable!() };
            family.execute("INSERT INTO users(id,email,given_name) VALUES(?1,'remote-owner@example.invalid','Owner')", &[Cell::uuid(owner)]).await.unwrap();
            family.execute("INSERT INTO workspaces(id,slug,name,kind) VALUES(?1,'remote-oidc-invite','Remote invitation','team')", &[Cell::uuid(workspace)]).await.unwrap();
            family.execute("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')", &[Cell::uuid(workspace), Cell::uuid(owner)]).await.unwrap();
            for (hash, email, expired) in [
                ("remote-accept", "remote-invited@example.invalid", false),
                ("remote-expired", "remote-expired@example.invalid", true),
                ("remote-conflict", "remote-conflict@example.invalid", false),
                ("remote-existing", "remote-owner@example.invalid", false),
                ("remote-unauthorized", "remote-unauthorized@example.invalid", false),
            ] {
                let expires = if expired { Cell::Integer(0) } else { Cell::instant(crate::db::identity::stored_now() + Duration::hours(1)).unwrap() };
                family.execute("INSERT INTO invitations(id,workspace_id,email,role,token_hash,invited_by,expires_at) VALUES(?1,?2,?3,'member',?4,?5,?6)", &[Cell::uuid(Uuid::now_v7()), Cell::uuid(workspace), Cell::text(email), Cell::text(hash), Cell::uuid(owner), expires]).await.unwrap();
            }
            tx.commit().await.unwrap();
            let defaults = crate::settings::catalog::SettingsValues::defaults("FVOCI").defaults_user;
            let license = crate::license::absent();
            let accepted = accept_invite_with_identity(&backend, &license, ProviderKey::Generic, &profile("remote-invited-sub"), &state("remote-accept"), None, &defaults).await.unwrap();
            let OidcResult::Session { user_id, .. } = accepted else { panic!("accepted invitation must issue a session") };
            let fresh = Backend::LibsqlRemote(std::sync::Arc::new(RemoteDatabase::from_test_driver(driver.database().await.unwrap(), std::num::NonZeroU32::MIN)));
            let mut read = fresh.begin_read().await.unwrap();
            let DbTransaction::SqliteFamily(family) = &mut read else { unreachable!() };
            let rows = family.query("SELECT u.email,m.role,l.provider_user_id,l.issuer,(SELECT count(*) FROM events WHERE verb='invitation.accepted'),(SELECT count(*) FROM audit_log WHERE verb='invitation.accepted'),(SELECT count(*) FROM events WHERE verb='identity.linked') FROM users u JOIN memberships m ON m.user_id=u.id JOIN identity_links l ON l.user_id=u.id WHERE u.id=?1 AND m.workspace_id=?2", &[Cell::uuid(user_id), Cell::uuid(workspace)]).await.unwrap();
            assert_eq!(rows.len(), 1);
            for (index, expected) in ["remote-invited@example.invalid", "member", "remote-invited-sub", "https://idp.example.invalid"].into_iter().enumerate() {
                assert_eq!(rows[0].cell(index).unwrap().string().unwrap(), expected);
            }
            for index in 4..7 { assert_eq!(rows[0].cell(index).unwrap().integer().unwrap(), 1); }
            read.rollback().await.unwrap();
            fresh.close().await.unwrap();
            for (hash, subject, code, demote) in [
                ("remote-accept", "remote-invited-sub", OidcErrorCode::InvitationInvalid, false),
                ("remote-expired", "remote-expired-sub", OidcErrorCode::InvitationInvalid, false),
                ("remote-conflict", "remote-invited-sub", OidcErrorCode::AlreadyLinked, false),
                ("remote-existing", "remote-unowned-sub", OidcErrorCode::InvitationInvalid, false),
                ("remote-unauthorized", "remote-unauthorized-sub", OidcErrorCode::InvitationInvalid, true),
            ] {
                if demote {
                    let mut tx = backend.begin_write().await.unwrap();
                    let DbTransaction::SqliteFamily(family) = &mut tx else { unreachable!() };
                    family.execute("UPDATE memberships SET role='member' WHERE workspace_id=?1 AND user_id=?2", &[Cell::uuid(workspace), Cell::uuid(owner)]).await.unwrap();
                    tx.commit().await.unwrap();
                }
                let before = effects(&backend).await;
                let refused = accept_invite_with_identity(&backend, &license, ProviderKey::Generic, &profile(subject), &state(hash), None, &defaults).await.unwrap();
                assert!(matches!(refused, OidcResult::Error { code: actual, mode: Some(Mode::Invite) } if actual == code));
                assert_eq!(effects(&backend).await, before);
            }
        }).catch_unwind().await;
        let close = backend.close().await;
        let finish = driver.finish().await;
        close.unwrap();
        let receipt = finish.unwrap();
        assert!(receipt.sqld_reaped && receipt.proxy_joined);
        std::fs::remove_dir_all(root).unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(active: &str) -> Keyring {
        Keyring::parse_named(
            &format!(r#"{{"a":"{}","b":"{}"}}"#, "11".repeat(32), "22".repeat(32)),
            active,
            "ENCRYPTION_KEYS",
        )
        .unwrap()
    }

    #[test]
    fn signed_state_binds_state_and_key() {
        let signed = sign_state(&ring("a"), "abc").unwrap();
        assert!(verify_signed_state(&ring("a"), "abc", Some(&signed)));
        // Rotation keeps it valid; another state or tampered MAC does not.
        assert!(verify_signed_state(&ring("b"), "abc", Some(&signed)));
        assert!(!verify_signed_state(&ring("a"), "abd", Some(&signed)));
        let mut tampered = signed.clone();
        let last = tampered.pop().unwrap();
        tampered.push(if last == '0' { '1' } else { '0' });
        assert!(!verify_signed_state(&ring("a"), "abc", Some(&tampered)));
        assert!(!verify_signed_state(&ring("a"), "abc", None));
        assert!(!verify_signed_state(&ring("a"), "abc", Some("abc")));
        let other =
            Keyring::parse_named(&format!(r#"{{"c":"{}"}}"#, "33".repeat(32)), "c", "X").unwrap();
        assert!(!verify_signed_state(&other, "abc", Some(&signed)));
    }

    #[test]
    fn subjects_and_domains() {
        let ws = Uuid::nil();
        assert_eq!(
            identity_subject(ProviderKey::Generic, "s", Some(ws)),
            format!("{ws}:s")
        );
        assert_eq!(identity_subject(ProviderKey::Google, "s", Some(ws)), "s");
        assert_eq!(identity_subject(ProviderKey::Generic, "s", None), "s");
        assert_eq!(email_domain("a@Corp.Example"), Some("corp.example".into()));
        assert_eq!(email_domain("@x"), None);
        assert_eq!(email_domain("x@"), None);
    }

    #[test]
    fn stored_state_is_strict() {
        let json = r#"{"provider":"google","mode":"login","nonce":"n","pkceVerifier":"v","expiresAtMs":1,"issuer":"i","clientId":"c","extra":1}"#;
        assert!(serde_json::from_str::<StoredState>(json).is_err());
    }
}
